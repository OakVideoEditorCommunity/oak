use super::*;
use oak_ai_tools::error::CODE_UNSUPPORTED;

/// Serialises the tests that touch a real CLI, so parallel `cargo test`
/// threads cannot interleave their temp libraries.
fn serial() -> std::sync::MutexGuard<'static, ()> {
	static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
	LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn temp_root(tag: &str) -> PathBuf {
	let dir = std::env::temp_dir().join(format!("oak-mem-test-{tag}-{}", std::process::id()));
	let _ = std::fs::remove_dir_all(&dir);
	std::fs::create_dir_all(&dir).expect("temp root");
	dir
}

fn bin() -> Option<PathBuf> {
	let path = resolve_bin();
	path.is_file().then_some(path)
}

#[test]
fn secret_gate_catches_what_the_cli_lets_through() {
	// Measured against memlite v0.1.0: these two were stored, rc=0.
	assert_eq!(
		find_secret("我的 key 是 sk-proj-abc123def456ghi789jkl012mno345"),
		Some(SecretKind::OpenAiStyleKey),
		"a sk-proj token must be refused here even though the CLI accepts it"
	);
	assert_eq!(
		find_secret("数据库口令 = hunter2correcthorse"),
		Some(SecretKind::AssignedSecret)
	);
	assert_eq!(
		find_secret("password=Sup3rS3cretValue"),
		Some(SecretKind::AssignedSecret)
	);
	// What the CLI already caught; the Rust gate must agree.
	assert_eq!(
		find_secret("身份证号 11010519900307123X 记一下"),
		Some(SecretKind::ChineseIdCard)
	);
	assert_eq!(
		find_secret("用户手机 13812345678"),
		Some(SecretKind::PhoneNumber)
	);
	assert_eq!(
		find_secret("token ghp_0123456789abcdefghijABCDEFGHIJ012345"),
		Some(SecretKind::GitHubToken)
	);
	assert_eq!(
		find_secret("AKIAIOSFODNN7EXAMPLE"),
		Some(SecretKind::AwsAccessKey)
	);
	assert_eq!(
		find_secret("-----BEGIN RSA PRIVATE KEY-----\nMIIE..."),
		Some(SecretKind::PrivateKeyBlock)
	);
	assert_eq!(
		find_secret("curl -H 'Authorization: Bearer abcdef'"),
		Some(SecretKind::AuthorizationHeader)
	);
}

#[test]
fn secret_gate_does_not_fire_on_editing_facts() {
	for clean in [
		"oak 导出默认用 H.264，代理文件放 /tmp/oak-proxy",
		"工程在 1920x1080 下渲染，fps=30",
		"sequence.set_format 的 fps 参数要传 {num,den}",
		"时间线 clip id 形如 n17，不是 uuid",
		"1296000 帧是 12 小时",
		"记忆库在 ~/.oak/memory/user",
	] {
		assert_eq!(find_secret(clean), None, "false positive on: {clean}");
	}
}

#[test]
fn library_dir_name_is_stable_and_filesystem_safe() {
	let a = library_dir_name("/home/u/项目 A/oak/project.oak");
	assert_eq!(a, library_dir_name("/home/u/项目 A/oak/project.oak"));
	assert!(
		a.chars()
			.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
		"{a}"
	);
	assert_ne!(a, library_dir_name("/home/u/项目 B/oak/project.oak"));
	assert!(library_dir_name("").starts_with("project-"));
	assert!(library_dir_name(&"x".repeat(200)).len() <= 49);
}

#[test]
fn truncation_keeps_the_head_and_names_the_tool() {
	let long = "字".repeat(RECALL_CONTENT_CHARS + 10);
	let cut = truncate(&long, RECALL_CONTENT_CHARS);
	assert!(cut.contains("memory.show"), "{cut}");
	assert_eq!(truncate("短内容", RECALL_CONTENT_CHARS), "短内容");
}

#[test]
fn missing_binary_is_a_structured_gap_not_a_panic() {
	let root = temp_root("nobin");
	let tools = MemoryTools::new(MemoryConfig::new(
		root.join("definitely-not-memlite"),
		root.join("oak-home"),
		Some("proj".into()),
	));
	let err = tools.recall(&json!({"query": "x"})).unwrap_err();
	assert_eq!(err.code, CODE_UNSUPPORTED);
	assert_eq!(err.gap.as_deref(), Some(GAP_MEMLITE_BINARY));
	assert!(
		err.message.contains("OAK_MEMLITE_BIN"),
		"install hint missing: {err}"
	);
}

#[test]
fn empty_query_and_empty_content_are_refused_before_spawning() {
	let root = temp_root("args");
	// A deliberately absent binary: argument checks must run first, so
	// the error is INVALID_PARAMS, not UNSUPPORTED.
	let tools = MemoryTools::new(MemoryConfig::new(
		root.join("no-bin"),
		root.join("oak-home"),
		None,
	));
	assert_eq!(
		tools.recall(&json!({"query": "   "})).unwrap_err().code,
		CODE_INVALID_PARAMS
	);
	assert_eq!(
		tools.remember(&json!({"content": ""})).unwrap_err().code,
		CODE_INVALID_PARAMS
	);
	assert_eq!(
		tools
			.remember(&json!({"content": "key sk-proj-aaaaaaaaaaaaaaaaaaaa"}))
			.unwrap_err()
			.code,
		CODE_INVALID_PARAMS
	);
}

#[test]
fn unknown_memory_tool_is_refused() {
	let tools = MemoryTools::new(MemoryConfig::new("/nonexistent", "/tmp", None));
	let err = execute(&tools, "memory.bogus", json!({})).unwrap_err();
	assert_eq!(err.code, oak_ai_tools::error::CODE_UNKNOWN_TOOL);
	assert_eq!(err.tool.as_deref(), Some("memory.bogus"));
}

/// The whole loop against the real CLI, in a private temp library.
#[test]
fn real_cli_roundtrip_in_a_temp_library() {
	let _guard = serial();
	let Some(bin) = bin() else {
		eprintln!("skipping: memlite binary not installed");
		return;
	};
	let root = temp_root("roundtrip");
	let tools = MemoryTools::new(
		MemoryConfig::new(bin, &root, Some("oak-test".into()))
			.with_timeout(Duration::from_secs(20)),
	);

	let stored = tools
		.remember(&json!({
			"content": "oak 面板的 memory.recall 默认只返回 5 条，正文截断到 400 字",
			"title": "recall 预算",
			"tags": "oak,memory"
		}))
		.expect("remember");
	assert_eq!(stored["tier"], "project");
	let id = stored["id"].as_str().expect("id").to_string();
	assert_eq!(id.len(), 12, "memlite ids are 12 hex chars: {id}");

	let recalled = tools
		.recall(&json!({"query": "recall 预算"}))
		.expect("recall");
	assert_eq!(
		recalled["backend"], "bm25",
		"BM25 is the only backend we accept"
	);
	let hits = recalled["hits"].as_array().expect("hits");
	assert_eq!(hits.len(), 1, "{recalled}");
	assert_eq!(hits[0]["id"].as_str().unwrap(), id);
	assert_eq!(hits[0]["tier"], "project");
	assert!(hits[0]["content"].as_str().unwrap().contains("400 字"));

	// Two tiers: a project library with no match falls back to user.
	let user_tools = MemoryTools::new(
		MemoryConfig::new(tools.config().bin(), &root, None).with_timeout(Duration::from_secs(20)),
	);
	user_tools
		.remember(&json!({"content": "用户偏好深色主题与中文界面", "title": "主题偏好"}))
		.expect("user remember");
	let merged = MemoryTools::new(
		MemoryConfig::new(tools.config().bin(), &root, Some("oak-test".into()))
			.with_timeout(Duration::from_secs(20)),
	)
	.recall(&json!({"query": "主题偏好", "limit": 3}))
	.expect("two-tier recall");
	let hits = merged["hits"].as_array().expect("hits");
	assert_eq!(
		merged["fallback"], "user",
		"second tier must be consulted: {merged}"
	);
	assert!(hits.iter().any(|h| h["tier"] == "user"), "{merged}");

	let shown = tools.show(&json!({"id": id})).expect("show");
	assert_eq!(shown["id"].as_str().unwrap(), id);
	assert_eq!(shown["tier"], "project");

	let listed = tools.list(&json!({"limit": 10})).expect("list");
	assert!(
		listed["entries"]
			.as_array()
			.unwrap()
			.iter()
			.any(|e| e["id"] == json!(id))
	);

	let status = tools.status(&json!({})).expect("status");
	assert_eq!(status["backend"], "bm25");

	let forgotten = tools.forget(&json!({"id": id})).expect("forget");
	assert_eq!(forgotten["deleted"].as_str().unwrap(), id);
	let err = tools.show(&json!({"id": id})).unwrap_err();
	assert_eq!(err.code, CODE_NOT_FOUND, "{err:?}");

	let _ = std::fs::remove_dir_all(&root);
}

/// The private config must carry no embedding block, and the user's
/// `~/.memlite/config.json` must stay untouched.
#[test]
fn private_config_isolates_the_user_embedding_settings() {
	let _guard = serial();
	let Some(bin) = bin() else {
		eprintln!("skipping: memlite binary not installed");
		return;
	};
	let root = temp_root("config");
	let user_config = home_dir().join(".memlite").join("config.json");
	let before = std::fs::read(&user_config).ok();

	let tools = MemoryTools::new(
		MemoryConfig::new(bin, &root, Some("p".into())).with_timeout(Duration::from_secs(20)),
	);
	tools
		.remember(&json!({"content": "配置隔离测试条目"}))
		.expect("remember");

	let written = std::fs::read_to_string(tools.config().config_path()).expect("private config");
	let parsed: Value = serde_json::from_str(&written).expect("json");
	assert!(
		parsed.get("data_dir").and_then(Value::as_str).is_some(),
		"{written}"
	);
	assert!(
		parsed.get("embedding").map(Value::is_null).unwrap_or(true),
		"the private config must not configure an embedding backend: {written}"
	);
	assert_eq!(
		std::fs::read(&user_config).ok(),
		before,
		"~/.memlite/config.json must never be written"
	);
	let _ = std::fs::remove_dir_all(&root);
}

/// Concurrency: the panel can fire several remembers in one turn. The CLI
/// serialises its append (measured: 8 parallel writes, no lost records),
/// and our wrapper must not add a shared lock that would serialise slower.
#[test]
fn parallel_remembers_do_not_lose_records() {
	let _guard = serial();
	let Some(bin) = bin() else {
		eprintln!("skipping: memlite binary not installed");
		return;
	};
	let root = temp_root("parallel");
	let handles: Vec<_> = (0..4)
		.map(|i| {
			let bin = bin.clone();
			let root = root.clone();
			std::thread::spawn(move || {
				let tools = MemoryTools::new(
					MemoryConfig::new(bin, root, Some("p".into()))
						.with_timeout(Duration::from_secs(20)),
				);
				tools.remember(
					&json!({"content": format!("并发写入第 {i} 条"), "title": format!("并发{i}")}),
				)
			})
		})
		.collect();
	for handle in handles {
		handle.join().expect("thread").expect("remember");
	}
	let tools = MemoryTools::new(
		MemoryConfig::new(bin, &root, Some("p".into())).with_timeout(Duration::from_secs(20)),
	);
	let listed = tools.list(&json!({"limit": 50})).expect("list");
	assert_eq!(
		listed["entries"].as_array().unwrap().len(),
		4,
		"every concurrent write must survive: {listed}"
	);
	let _ = std::fs::remove_dir_all(&root);
}

/// A fake CLI records its argv and environment, so the invocation
/// contract is asserted directly instead of inferred: `--bm25` pinned,
/// `--data-dir` passed, and `MEMLITE_CONFIG` pointing at Oak's own file.
#[cfg(unix)]
#[test]
fn every_call_pins_bm25_and_the_private_config() {
	let root = temp_root("fakecli");
	let fake = root.join("fake-memlite.sh");
	let record = root.join("calls.log");
	std::fs::write(
		&fake,
		format!(
			"#!/bin/sh\n\
				 echo \"CFG=$MEMLITE_CONFIG ARGS=$*\" >> {log}\n\
				 echo '{{ \"backend\": \"bm25\", \"fallback\": \"\", \"hits\": [] }}'\n",
			log = record.display()
		),
	)
	.expect("write fake cli");
	let mut perms = std::fs::metadata(&fake).expect("meta").permissions();
	std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
	std::fs::set_permissions(&fake, perms).expect("chmod");

	let cfg = MemoryConfig::new(fake.clone(), root.join("oak-home"), Some("p".into()));
	let private_config = cfg.config_path().to_path_buf();
	let tools = MemoryTools::new(cfg);
	let _ = tools.recall(&json!({"query": "任意词"}));

	let log = std::fs::read_to_string(&record).expect("fake cli must have run");
	let line = log.lines().next().unwrap_or_default();
	assert!(
		line.contains(&format!("CFG={}", private_config.display())),
		"MEMLITE_CONFIG must be Oak's private file, got: {line}"
	);
	assert!(
		line.contains("--bm25"),
		"the lexical path must be pinned: {line}"
	);
	assert!(
		line.contains("--data-dir"),
		"the data dir must be explicit: {line}"
	);
	let written = std::fs::read_to_string(&private_config).expect("private config");
	assert!(
		!written.contains("api_base") && !written.contains("embedding\": {"),
		"the private config must not configure an embedding endpoint: {written}"
	);
	let _ = std::fs::remove_dir_all(&root);
}

/// A CLI that never answers must surface as the structured timeout, not
/// as a hung panel turn and not as a panic.
#[cfg(unix)]
#[test]
fn slow_cli_times_out_as_a_structured_error() {
	let root = temp_root("slowcli");
	let fake = root.join("slow-memlite.sh");
	std::fs::write(&fake, "#!/bin/sh\nsleep 30\n").expect("write slow cli");
	let mut perms = std::fs::metadata(&fake).expect("meta").permissions();
	std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
	std::fs::set_permissions(&fake, perms).expect("chmod");

	let tools = MemoryTools::new(
		MemoryConfig::new(fake, root.join("oak-home"), Some("p".into()))
			.with_timeout(Duration::from_millis(300)),
	);
	let started = Instant::now();
	let err = tools.recall(&json!({"query": "任意词"})).unwrap_err();
	assert_eq!(err.code, CODE_STATE, "{err:?}");
	assert!(
		err.message.contains("未返回") && err.message.contains("已终止"),
		"{err}"
	);
	assert!(
		started.elapsed() < Duration::from_secs(5),
		"the deadline must bite"
	);
	let _ = std::fs::remove_dir_all(&root);
}

/// Five secret shapes: refused, and the library stays empty afterwards —
/// "refused" is proven by the absence of the record, not by the message.
#[test]
fn five_secret_shapes_are_refused_and_never_stored() {
	let _guard = serial();
	let Some(bin) = bin() else {
		eprintln!("skipping: memlite binary not installed");
		return;
	};
	let root = temp_root("secrets");
	let tools = MemoryTools::new(
		MemoryConfig::new(bin, &root, Some("p".into())).with_timeout(Duration::from_secs(20)),
	);
	let cases = [
		(
			"sk-proj token",
			"我的 key 是 sk-proj-abc123def456ghi789jkl012mno345",
		),
		("口令赋值", "数据库口令 = hunter2correcthorse"),
		(
			"PEM 私钥",
			"-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----",
		),
		("身份证", "记录一下 11010519900307123X"),
		("手机号", "用户手机 13812345678"),
	];
	for (label, content) in cases {
		let err = tools
			.remember(&json!({ "content": content }))
			.expect_err(&format!("{label} must be refused"));
		assert_eq!(err.code, CODE_INVALID_PARAMS, "{label}: {err:?}");
		assert!(err.message.contains("位置指针"), "{label}: {err}");
	}
	// Nothing reached disk: no library was created, and a read finds none.
	assert!(
		!tools.config().project_dir().unwrap().exists(),
		"a refused remember must not create the library"
	);
	let listed = tools.list(&json!({ "limit": 10 })).expect("list");
	assert_eq!(
		listed["entries"].as_array().unwrap().len(),
		0,
		"no entry may exist after five refusals: {listed}"
	);
	let _ = std::fs::remove_dir_all(&root);
}
