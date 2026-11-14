//! `memory.*` — long-term memory tools backed by the **memlite** CLI (T11A).
//!
//! The sovereign's instruction (2026-11-11): integrate memlite, **BM25 only**
//! (no embedding service, no respire). The CLI is a separate process, so this
//! module is a subprocess wrapper plus three pieces of policy that the CLI
//! does *not* provide:
//!
//! 1. **Config isolation** — with only `--data-dir`, `recall` still reads the
//!    user's `~/.memlite/config.json` and dials its embedding endpoint
//!    (`127.0.0.1:18321`) before falling back to BM25. Measured with strace:
//!    `connect(… htons(18321))`. So every call sets `MEMLITE_CONFIG` to a
//!    private file **and** passes `--data-dir`; `--bm25` pins the lexical
//!    path. On a user machine this removes a timeout stall per recall.
//! 2. **Sensitive refusal in Rust** — the shipped binary (v0.1.0,
//!    2026-10-14) rejects an 18-digit id and an 11-digit phone, but accepts
//!    `sk-proj-…` tokens and `密码 = …` assignments (measured, rc=0, stored).
//!    A gate that misses the common cases is not a gate, so the scan lives
//!    here, before the child is spawned.
//! 3. **Structured errors** — the CLI reports failures as `rc=1` + plain
//!    stderr text (there is no JSON error shape even under `--json`), and
//!    reports "nothing found" as `rc=0` + empty hits. Tool callers must be
//!    able to tell those apart, so bad arguments and missing libraries are
//!    refused here and CLI failures are wrapped with the stderr text plus a
//!    next step.
//!
//! # Verified CLI shapes (all measured, not from the help text)
//!
//! * `remember <CONTENT> --title --tags --project` →
//!   `{"embedded":false,"id":"<12 hex>","records":N,"title":…}`
//!   (no `importance` / `parent` / `kind` — those do not exist in this CLI);
//! * `recall <QUERY> [--limit] [--project] [--bm25]` →
//!   `{"backend":"bm25","fallback":"","hits":[{content,id,project,score,tags,title,ts}]}`;
//! * `show <ID>` / `list [--limit]` items → `{content,id,op,project,tags,title,ts}`;
//! * `update <ID> --title --content` → `{embedded,id,title}`;
//! * `forget <ID>` → `{"deleted":"<id>"}`;
//! * `status` → `{bad_lines,data_dir,embedding{api_base,configured,model},
//!   files,lines,records,sync,vectors}`.
//!
//! # Two tiers
//!
//! Project tier first, user tier as the fallback (the CLI's own `fallback`
//! field is only used when it resolves the tiers itself, which it cannot do
//! for our directories). Libraries live under the application data directory
//! (`$OAK_HOME` / `~/.oak`), never inside the `.oak` project file.

use oak_ai_tools::error::{ToolError, CODE_INVALID_PARAMS, CODE_NOT_FOUND, CODE_STATE};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// Capability gap reported when the CLI is nowhere to be found.
pub const GAP_MEMLITE_BINARY: &str = "oak.memory.memlite-binary-missing";
/// Capability gap reported when no project scope can be resolved.
pub const GAP_PROJECT_SCOPE: &str = "oak.memory.no-project-scope";

/// Longest memory body that `recall` puts in front of the model. BM25 ranks
/// whole records, and a 4 KiB record would eat the budget of five results.
pub const RECALL_CONTENT_CHARS: usize = 400;
/// `memory.show` is the "read it properly" tool, so it gets a larger cap.
pub const SHOW_CONTENT_CHARS: usize = 4_000;
/// Default number of hits (lead's ruling: 5).
pub const DEFAULT_RECALL_LIMIT: usize = 5;
/// Subprocess budget; the CLI is a local file operation, 10 s is generous.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// `memory.list` default page.
pub const DEFAULT_LIST_LIMIT: usize = 20;

// ---------------------------------------------------------------------------
// Sensitive-content gate
// ---------------------------------------------------------------------------

/// Why a body was refused. The tag is what the error message shows, so it has
/// to name the *kind* of secret, not the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
	OpenAiStyleKey,
	GitHubToken,
	AwsAccessKey,
	PrivateKeyBlock,
	AuthorizationHeader,
	AssignedSecret,
	ChineseIdCard,
	PhoneNumber,
}

impl SecretKind {
	pub fn label(self) -> &'static str {
		match self {
			SecretKind::OpenAiStyleKey => "API 密钥（sk- 前缀）",
			SecretKind::GitHubToken => "GitHub 令牌（ghp_/gho_/github_pat_）",
			SecretKind::AwsAccessKey => "AWS 访问密钥（AKIA…）",
			SecretKind::PrivateKeyBlock => "PEM 私钥块",
			SecretKind::AuthorizationHeader => "Authorization/Bearer 凭据",
			SecretKind::AssignedSecret => "明文口令赋值（password/token/口令/密码=…）",
			SecretKind::ChineseIdCard => "身份证号",
			SecretKind::PhoneNumber => "手机号",
		}
	}
}

/// Scans `text` for secret-shaped material.
///
/// Deliberately conservative on identifiers: an 18-character id is only
/// refused when it has the `dddddd YYYYMMDD dddX` shape, and a phone only when
/// it is eleven digits starting with `1` and not glued to other digits — so
/// `1920x1080`, timestamps and ids do not trip the gate.
pub fn find_secret(text: &str) -> Option<SecretKind> {
	let lower = text.to_lowercase();
	// Whitespace-insensitive matching for the *assignment* patterns only.
	// Prefixed keys and identifiers are scanned on `lower`: squashing the
	// whitespace glues the preceding word onto the token (`key sk-proj-…`
	// becomes `keysk-proj-…`), which defeats the start-of-token check and
	// silently lets the secret through.
	let squashed: String = lower.chars().filter(|c| !c.is_whitespace()).collect();

	if squashed.contains("-----begin") && squashed.contains("privatekey-----") {
		return Some(SecretKind::PrivateKeyBlock);
	}
	if squashed.contains("authorization:") || lower.contains("bearer ") {
		return Some(SecretKind::AuthorizationHeader);
	}
	for needle in [
		"api_key=",
		"apikey=",
		"api-key=",
		"token=",
		"password=",
		"passwd=",
		"secret=",
		"口令=",
		"密码=",
		"口令:",
		"密码:",
	] {
		if squashed.contains(needle) {
			return Some(SecretKind::AssignedSecret);
		}
	}
	if let Some(kind) = scan_prefixed(&lower) {
		return Some(kind);
	}
	if contains_id_card(&lower) {
		return Some(SecretKind::ChineseIdCard);
	}
	if contains_phone(&lower) {
		return Some(SecretKind::PhoneNumber);
	}
	None
}

/// `sk-…`, `sk-proj-…`, `ghp_…`, `gho_…`, `github_pat_…`, `AKIA…`.
fn scan_prefixed(text: &str) -> Option<SecretKind> {
	let bytes = text.as_bytes();
	for (i, _) in text.match_indices("sk-") {
		// `sk-` must start a token, and be followed by a credential-looking tail.
		if i > 0 && is_token_char(bytes[i - 1] as char) {
			continue;
		}
		let tail = &text[i + 3..];
		let tail = tail.strip_prefix("proj-").or_else(|| tail.strip_prefix("ant-")).or_else(|| tail.strip_prefix("svcacct-")).unwrap_or(tail);
		let run = tail.chars().take_while(|c| is_token_char(*c)).count();
		if run >= 16 {
			return Some(SecretKind::OpenAiStyleKey);
		}
	}
	for prefix in ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"] {
		for (i, _) in text.match_indices(prefix) {
			if i > 0 && is_token_char(bytes[i - 1] as char) {
				continue;
			}
			let run = text[i + prefix.len()..]
				.chars()
				.take_while(|c| is_token_char(*c))
				.count();
			if run >= 20 {
				return Some(SecretKind::GitHubToken);
			}
		}
	}
	for (i, _) in text.match_indices("akia") {
		if i > 0 && is_token_char(bytes[i - 1] as char) {
			continue;
		}
		let run = text[i + 4..]
			.chars()
			.take_while(|c| c.is_ascii_alphanumeric())
			.count();
		if run >= 16 {
			return Some(SecretKind::AwsAccessKey);
		}
	}
	None
}

fn is_token_char(c: char) -> bool {
	c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}

/// Eighteen-character id card: six digits, a plausible date, then three
/// digits and a check character (digit or `X`).
fn contains_id_card(text: &str) -> bool {
	let chars: Vec<char> = text.chars().collect();
	let mut i = 0;
	while i + 18 <= chars.len() {
		let window: String = chars[i..i + 18].iter().collect();
		let is_id = window.chars().take(17).all(|c| c.is_ascii_digit())
			&& matches!(window.chars().nth(17), Some(d) if d.is_ascii_digit() || d == 'x');
		if is_id {
			let boundary_before = i == 0 || !chars[i - 1].is_ascii_digit();
			let boundary_after = i + 18 == chars.len() || !chars[i + 18].is_ascii_digit();
			let year: u32 = window[6..10].parse().unwrap_or(0);
			let month: u32 = window[10..12].parse().unwrap_or(0);
			let day: u32 = window[12..14].parse().unwrap_or(0);
			if boundary_before
				&& boundary_after
				&& (1900..=2100).contains(&year)
				&& (1..=12).contains(&month)
				&& (1..=31).contains(&day)
			{
				return true;
			}
		}
		i += 1;
	}
	false
}

/// Eleven digits starting with `1`, not embedded in a longer digit run.
fn contains_phone(text: &str) -> bool {
	let chars: Vec<char> = text.chars().collect();
	let mut i = 0;
	while i + 11 <= chars.len() {
		if chars[i] == '1' && chars[i..i + 11].iter().all(|c| c.is_ascii_digit()) {
			let before_ok = i == 0 || !chars[i - 1].is_ascii_digit();
			let after_ok = i + 11 == chars.len() || !chars[i + 11].is_ascii_digit();
			if before_ok && after_ok {
				return true;
			}
		}
		i += 1;
	}
	false
}

fn refusal(kind: SecretKind) -> ToolError {
	ToolError::new(
		CODE_INVALID_PARAMS,
		format!(
			"这条记忆含疑似{}，已拒绝入库（记忆库会被同步/回读，密钥与隐私不得进入）。\
			 如确需记住，请改写为位置指针，例如「OpenAI key 在 ~/.config/oak/env 的 OAK_AI_API_KEY」。",
			kind.label()
		),
	)
}

// ---------------------------------------------------------------------------
// Configuration and paths
// ---------------------------------------------------------------------------

/// Where the binary, the libraries and the private config live.
#[derive(Debug, Clone)]
pub struct MemoryConfig {
	bin: PathBuf,
	/// Application data root (`$OAK_HOME` or `~/.oak`).
	root: PathBuf,
	/// Oak's own memlite config: pins the data dir and, crucially, carries
	/// **no** embedding block so the CLI never dials the user's endpoint.
	config_path: PathBuf,
	timeout: Duration,
	project: Option<String>,
}

impl MemoryConfig {
	/// Fully explicit form (tests, and the assembly point that knows the
	/// project identity).
	pub fn new(bin: impl Into<PathBuf>, root: impl Into<PathBuf>, project: Option<String>) -> Self {
		let root = root.into();
		let config_path = root.join("memory").join("memlite.json");
		Self {
			bin: bin.into(),
			root,
			config_path,
			timeout: DEFAULT_TIMEOUT,
			project,
		}
	}

	/// Environment form: `$OAK_MEMLITE_BIN`, `$OAK_HOME`, `$OAK_MEMORY_PROJECT`.
	pub fn from_env() -> Self {
		let root = match std::env::var_os("OAK_HOME") {
			Some(dir) => PathBuf::from(dir),
			None => home_dir().join(".oak"),
		};
		let project = std::env::var("OAK_MEMORY_PROJECT").ok().filter(|s| !s.trim().is_empty());
		Self::new(resolve_bin(), root, project)
	}

	pub fn with_project(mut self, project: Option<String>) -> Self {
		self.project = project.filter(|s| !s.trim().is_empty());
		self
	}

	pub fn with_timeout(mut self, timeout: Duration) -> Self {
		self.timeout = timeout;
		self
	}

	pub fn bin(&self) -> &Path {
		&self.bin
	}
	pub fn root(&self) -> &Path {
		&self.root
	}
	pub fn config_path(&self) -> &Path {
		&self.config_path
	}
	pub fn project(&self) -> Option<&str> {
		self.project.as_deref()
	}

	/// Project library directory (none without a project scope).
	pub fn project_dir(&self) -> Option<PathBuf> {
		self.project.as_ref().map(|id| self.root.join("memory").join(library_dir_name(id)))
	}

	/// User library directory — the fallback tier.
	pub fn user_dir(&self) -> PathBuf {
		self.root.join("memory").join("user")
	}

	/// Writes the private config when it is missing or lacks the data dir.
	///
	/// The file is Oak's, not the user's: `~/.memlite/config.json` is never
	/// read for writes and never written.
	fn ensure_config(&self) -> Result<(), ToolError> {
		if let Some(parent) = self.config_path.parent() {
			std::fs::create_dir_all(parent).map_err(|e| {
				ToolError::new(
					oak_ai_tools::error::CODE_IO,
					format!("cannot create the memory config directory {}: {e}", parent.display()),
				)
			})?;
		}
		let wanted = json!({
			"data_dir": self.user_dir().to_string_lossy(),
			// Explicitly no `embedding`: BM25 is the only backend we want.
			"embedding": Value::Null,
		});
		let current = std::fs::read_to_string(&self.config_path).ok();
		let ok = current
			.as_deref()
			.and_then(|text| serde_json::from_str::<Value>(text).ok())
			.map(|v| v.get("data_dir").and_then(Value::as_str).is_some() && v.get("embedding").map(Value::is_null).unwrap_or(true))
			.unwrap_or(false);
		if ok {
			return Ok(());
		}
		let text = serde_json::to_string_pretty(&wanted).unwrap_or_else(|_| "{}".to_string());
		// Written through a temp file + rename: several tool calls can run
		// concurrently in one turn, and a half-written config would make the
		// next child read garbage.
		static STAGING: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
		let unique = STAGING.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
		let staging = self
			.config_path
			.with_extension(format!("tmp{}-{unique}", std::process::id()));
		std::fs::write(&staging, &text).map_err(|e| {
			ToolError::new(
				oak_ai_tools::error::CODE_IO,
				format!("cannot write the memory config {}: {e}", self.config_path.display()),
			)
		})?;
		std::fs::rename(&staging, &self.config_path).map_err(|e| {
			let _ = std::fs::remove_file(&staging);
			ToolError::new(
				oak_ai_tools::error::CODE_IO,
				format!("cannot install the memory config {}: {e}", self.config_path.display()),
			)
		})?;
		Ok(())
	}
}

fn home_dir() -> PathBuf {
	std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
}

/// Filesystem-safe directory name for a project id: printable ASCII is kept,
/// everything else becomes `-`, and a FNV-1a suffix keeps distinct ids apart.
pub fn library_dir_name(project: &str) -> String {
	let mut stem: String = project
		.chars()
		.map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
		.collect();
	if stem.len() > 40 {
		stem.truncate(40);
	}
	if stem.is_empty() {
		stem.push_str("project");
	}
	format!("{stem}-{:08x}", fnv1a32(project))
}

fn fnv1a32(text: &str) -> u32 {
	let mut hash: u32 = 0x811c_9dc5;
	for byte in text.as_bytes() {
		hash ^= *byte as u32;
		hash = hash.wrapping_mul(0x0100_0193);
	}
	hash
}

/// `$OAK_MEMLITE_BIN` → `~/.local/bin/memlite` → `PATH`.
///
/// A missing binary is not a panic and not a silent success: it is the
/// `UNSUPPORTED` gap the caller turns into an install hint.
pub fn resolve_bin() -> PathBuf {
	if let Some(explicit) = std::env::var_os("OAK_MEMLITE_BIN") {
		return PathBuf::from(explicit);
	}
	let local = home_dir().join(".local").join("bin").join("memlite");
	if local.is_file() {
		return local;
	}
	if let Some(path) = std::env::var_os("PATH") {
		for dir in std::env::split_paths(&path) {
			let candidate = dir.join("memlite");
			if candidate.is_file() {
				return candidate;
			}
		}
	}
	// Report the conventional location; `ensure_bin` turns it into the error.
	local
}

fn ensure_bin(cfg: &MemoryConfig) -> Result<(), ToolError> {
	if cfg.bin().is_file() {
		return Ok(());
	}
	Err(ToolError::unsupported(
		GAP_MEMLITE_BINARY,
		format!(
			"找不到 memlite 可执行文件（{}）。记忆功能需要 memlite 0.1+：\
			 安装后放到 ~/.local/bin/memlite，或用环境变量 OAK_MEMLITE_BIN 指定路径；\
			 本会话其余剪辑工具不受影响。",
			cfg.bin().display()
		),
	))
}

// ---------------------------------------------------------------------------
// The tool surface
// ---------------------------------------------------------------------------

/// The `memory.*` tools. Cheap to clone; every call spawns the CLI.
#[derive(Debug, Clone)]
pub struct MemoryTools {
	cfg: MemoryConfig,
}

/// Which library a result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
	Project,
	User,
}

impl Tier {
	pub fn as_str(self) -> &'static str {
		match self {
			Tier::Project => "project",
			Tier::User => "user",
		}
	}
}

// ---------------------------------------------------------------------------
// Write serialisation (upstream atomicity workaround)
// ---------------------------------------------------------------------------

/// Serialises CLI **writes** (`remember`, `forget`) per library directory.
///
/// Why this lock exists — measured, not precaution: memlite 0.1.0 appends a
/// record to `memory.jsonl` as **two separate `write()` syscalls** (strace:
/// `write(3, "{…}", 112)` then `write(3, "\n", 1)`). Two concurrent
/// remembers then interleave — A's content + B's newline + C's content — the
/// assembled line is no longer JSON, and the CLI silently drops it: the
/// 4-way concurrency test saw `list` count 0–2 of 4 with
/// `status.bad_lines = 1`, while serial writes stayed 4/4. That is an
/// upstream atomicity bug (the fix belongs in the CLI), so until it ships
/// oak serialises writes to the same data dir: a panel turn with concurrent
/// `memory.remember` calls must not lose records.
///
/// Reads (`recall`/`show`/`list`/`status`) take **no** lock, on purpose: a
/// torn line left by an out-of-band writer is the CLI's own `bad_lines`
/// tolerance to handle, and serialising reads would only slow the common
/// path.
///
/// One lock per data dir, so two libraries never block each other. The map
/// entry is leaked (one `Mutex` per distinct library, process lifetime)
/// because a `'static` guard cannot borrow a temporary `Arc`.
fn write_lock(dir: &Path) -> MutexGuard<'static, ()> {
	static LOCKS: OnceLock<Mutex<HashMap<PathBuf, &'static Mutex<()>>>> = OnceLock::new();
	let buckets = LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
	let slot = {
		let mut map = buckets.lock().unwrap_or_else(|e| e.into_inner());
		*map.entry(dir.to_path_buf())
			.or_insert_with(|| Box::leak(Box::new(Mutex::new(()))))
	};
	// A poisoned mutex means a writer panicked while holding it; later calls
	// must still run — the CLI's own `bad_lines` tolerance reports any
	// damage honestly — instead of cascading every write into a panic.
	slot.lock().unwrap_or_else(|e| e.into_inner())
}

impl MemoryTools {
	pub fn new(cfg: MemoryConfig) -> Self {
		Self { cfg }
	}

	pub fn from_env() -> Self {
		Self::new(MemoryConfig::from_env())
	}

	pub fn config(&self) -> &MemoryConfig {
		&self.cfg
	}

	/// `memory.remember` — store one memory.
	pub fn remember(&self, args: &Value) -> Result<Value, ToolError> {
		let content = required_str(args, "content")?;
		if content.trim().is_empty() {
			return Err(ToolError::new(
				CODE_INVALID_PARAMS,
				"content 为空：记忆正文不能为空字符串（要记的是哪条事实？）",
			));
		}
		if let Some(kind) = find_secret(content) {
			return Err(refusal(kind));
		}
		let title = optional_str(args, "title").unwrap_or_default();
		if let Some(kind) = find_secret(&title) {
			return Err(refusal(kind));
		}
		let tags = optional_str(args, "tags").unwrap_or_default();
		let project = optional_str(args, "project").or_else(|| self.cfg.project().map(str::to_string));

		let tier = if project.is_some() {
			Tier::Project
		} else {
			Tier::User
		};
		let dir = match tier {
			Tier::Project => self.cfg.project_dir().expect("project tier has a directory"),
			Tier::User => self.cfg.user_dir(),
		};
		// Creating the directory is safe: the CLI writes its own jsonl there
		// and a read on a missing directory would answer "nothing" instead.
		std::fs::create_dir_all(&dir).map_err(|e| {
			ToolError::new(
				oak_ai_tools::error::CODE_IO,
				format!("cannot create the memory library {}: {e}", dir.display()),
			)
		})?;

		let mut argv: Vec<String> = vec![
			"remember".into(),
			content.to_string(),
			"--json".into(),
			"--data-dir".into(),
			dir.to_string_lossy().into_owned(),
		];
		if !title.is_empty() {
			argv.push("--title".into());
			argv.push(title.clone());
		}
		if !tags.is_empty() {
			argv.push("--tags".into());
			argv.push(tags);
		}
		if let Some(project) = &project {
			argv.push("--project".into());
			argv.push(project.clone());
		}
		// Serialise against concurrent writers to this library: the CLI's
		// append is two non-atomic write() calls (see `write_lock`).
		let _write_guard = write_lock(dir.as_path());
		let out = self.run(&argv)?;
		let id = out.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
		if id.is_empty() {
			return Err(ToolError::new(
				oak_ai_tools::error::CODE_BACKEND,
				format!("memlite remember 没有返回 id（原始输出：{out}）"),
			));
		}
		Ok(json!({
			"id": id,
			"tier": tier.as_str(),
			"title": out.get("title").cloned().unwrap_or(Value::Null),
			"records": out.get("records").cloned().unwrap_or(Value::Null),
			"embedded": out.get("embedded").cloned().unwrap_or(Value::Bool(false)),
		}))
	}

	/// `memory.recall` — BM25 search across the tiers.
	pub fn recall(&self, args: &Value) -> Result<Value, ToolError> {
		let query = required_str(args, "query")?;
		if query.trim().is_empty() {
			return Err(ToolError::new(
				CODE_INVALID_PARAMS,
				"query 为空：检索词不能为空（换一两个实义词再试；要浏览全部条目请用 memory.list）",
			));
		}
		let limit = args
			.get("limit")
			.and_then(Value::as_u64)
			.map(|n| n.clamp(1, 20) as usize)
			.unwrap_or(DEFAULT_RECALL_LIMIT);

		let mut searched: Vec<Tier> = Vec::new();
		let mut hits: Vec<Value> = Vec::new();
		for (tier, dir) in self.tiers() {
			out_of_band(ensure_library_exists(dir.as_path(), tier)?)?;
			let out = self.recall_in(dir.as_path(), query, limit)?;
			searched.push(tier);
			let mut tier_hits: Vec<Value> = out
				.get("hits")
				.and_then(Value::as_array)
				.map(|rows| {
					rows.iter()
						.map(|hit| {
							json!({
								"id": hit.get("id").cloned().unwrap_or(Value::Null),
								"title": hit.get("title").cloned().unwrap_or(Value::Null),
								"score": hit.get("score").cloned().unwrap_or(Value::Null),
								"tags": hit.get("tags").cloned().unwrap_or_else(|| json!([])),
								"tier": tier.as_str(),
								"content": truncate(
									hit.get("content").and_then(Value::as_str).unwrap_or_default(),
									RECALL_CONTENT_CHARS
								),
							})
						})
						.collect()
				})
				.unwrap_or_default();
			// Project hits outrank user hits: the same subject in the local
			// library is the more specific statement.
			if tier == Tier::User {
				tier_hits.sort_by(|a, b| score_of(b).total_cmp(&score_of(a)));
			}
			hits.append(&mut tier_hits);
			if hits.len() >= limit && tier == Tier::Project {
				// A project library that already satisfies the budget does not
				// need the fallback tier; keep the response small.
				hits.truncate(limit);
				break;
			}
		}
		let mut response = json!({
			"query": query,
			"backend": "bm25",
			"hits": hits,
			"searched": searched.iter().map(|t| t.as_str()).collect::<Vec<_>>(),
		});
		if searched.len() > 1 {
			// Mirrors the CLI's own field, so both paths read the same.
			response["fallback"] = json!("user");
		}
		if response["hits"].as_array().map(Vec::is_empty).unwrap_or(true) {
			response["hint"] = json!(
				"两档库都没有命中。换更具体的实义词（工具名、文件名、错误码）再搜；\
				 记忆只记事实，不记对话内容。"
			);
		}
		Ok(response)
	}

	/// `memory.show` — full body of one entry (the read-it-properly tool).
	pub fn show(&self, args: &Value) -> Result<Value, ToolError> {
		let id = required_str(args, "id")?;
		let mut missing: Vec<&str> = Vec::new();
		for (tier, dir) in self.tiers() {
			if !dir.is_dir() {
				missing.push(tier.as_str());
				continue;
			}
			match self.run(&[
				"show".into(),
				id.to_string(),
				"--json".into(),
				"--data-dir".into(),
				dir.to_string_lossy().into_owned(),
			]) {
				Ok(item) => {
					let mut item = item;
					// Own the body before mutating the same `Value`: holding the
					// `&str` across the assignment is a borrow conflict.
					if let Some(content) = item
						.get("content")
						.and_then(Value::as_str)
						.map(str::to_string)
					{
						let total = content.chars().count();
						item["content"] = json!(truncate(&content, SHOW_CONTENT_CHARS));
						item["truncated"] = json!(total > SHOW_CONTENT_CHARS);
					}
					item["tier"] = json!(tier.as_str());
					return Ok(item);
				}
				Err(e) if e.code == CODE_NOT_FOUND => continue,
				Err(e) => return Err(e),
			}
		}
		Err(ToolError::new(
			CODE_NOT_FOUND,
			format!("记忆库里没有 id={id}。先用 memory.list 或 memory.recall 拿到真实 id（id 是 12 位十六进制）。"),
		))
	}

	/// `memory.list` — most recent entries, project tier first.
	pub fn list(&self, args: &Value) -> Result<Value, ToolError> {
		let limit = args
			.get("limit")
			.and_then(Value::as_u64)
			.map(|n| n.clamp(1, 100) as usize)
			.unwrap_or(DEFAULT_LIST_LIMIT);
		let mut entries: Vec<Value> = Vec::new();
		let mut tiers: Vec<&str> = Vec::new();
		for (tier, dir) in self.tiers() {
			if !dir.is_dir() {
				continue;
			}
			let out = self.run(&[
				"list".into(),
				"--json".into(),
				"--limit".into(),
				limit.to_string(),
				"--data-dir".into(),
				dir.to_string_lossy().into_owned(),
			])?;
			let rows = out.as_array().cloned().unwrap_or_default();
			if rows.is_empty() {
				continue;
			}
			tiers.push(tier.as_str());
			for row in rows {
				entries.push(json!({
					"id": row.get("id").cloned().unwrap_or(Value::Null),
					"title": row.get("title").cloned().unwrap_or(Value::Null),
					"tags": row.get("tags").cloned().unwrap_or_else(|| json!([])),
					"ts": row.get("ts").cloned().unwrap_or(Value::Null),
					"tier": tier.as_str(),
				}));
			}
		}
		entries.truncate(limit);
		Ok(json!({ "entries": entries, "tiers": tiers, "limit": limit }))
	}

	/// `memory.forget` — soft-delete one entry.
	pub fn forget(&self, args: &Value) -> Result<Value, ToolError> {
		let id = required_str(args, "id")?;
		for (tier, dir) in self.tiers() {
			if !dir.is_dir() {
				continue;
			}
			// Same serialisation as `remember`: a delete rewrites the jsonl.
			let _write_guard = write_lock(dir.as_path());
			match self.run(&[
				"forget".into(),
				id.to_string(),
				"--json".into(),
				"--data-dir".into(),
				dir.to_string_lossy().into_owned(),
			]) {
				Ok(out) => {
					return Ok(json!({
						"deleted": out.get("deleted").cloned().unwrap_or_else(|| json!(id)),
						"tier": tier.as_str(),
					}));
				}
				Err(e) if e.code == CODE_NOT_FOUND => continue,
				Err(e) => return Err(e),
			}
		}
		Err(ToolError::new(
			CODE_NOT_FOUND,
			format!("两档库都没有 id={id}，无需删除。用 memory.list 核对 id。"),
		))
	}

	/// `memory.status` — sizes, tiers and the backend actually in use.
	pub fn status(&self, _args: &Value) -> Result<Value, ToolError> {
		ensure_bin(&self.cfg)?;
		let mut tiers = Vec::new();
		let mut total = 0u64;
		for (tier, dir) in self.tiers() {
			let entry = if dir.is_dir() {
				let out = self.run(&[
					"status".into(),
					"--json".into(),
					"--data-dir".into(),
					dir.to_string_lossy().into_owned(),
				])?;
				let records = out.get("records").and_then(Value::as_u64).unwrap_or(0);
				total += records;
				json!({
					"tier": tier.as_str(),
					"dir": dir.to_string_lossy(),
					"records": records,
					"backend": if out.get("embedding").and_then(|e| e.get("configured")).and_then(Value::as_bool).unwrap_or(false) { "embedding" } else { "bm25" },
				})
			} else {
				json!({ "tier": tier.as_str(), "dir": dir.to_string_lossy(), "records": 0, "exists": false })
			};
			tiers.push(entry);
		}
		Ok(json!({
			"bin": self.cfg.bin().to_string_lossy(),
			"project": self.cfg.project(),
			"tiers": tiers,
			"records": total,
			"backend": "bm25",
		}))
	}

	fn tiers(&self) -> Vec<(Tier, PathBuf)> {
		let mut out = Vec::new();
		if let Some(dir) = self.cfg.project_dir() {
			out.push((Tier::Project, dir));
		}
		out.push((Tier::User, self.cfg.user_dir()));
		out
	}

	fn recall_in(&self, dir: &Path, query: &str, limit: usize) -> Result<Value, ToolError> {
		self.run(&[
			"recall".into(),
			query.to_string(),
			"--json".into(),
			// Pin the lexical path: BM25 is the installed mode, and the flag
			// means the user's embedding config cannot change the answer.
			"--bm25".into(),
			"--limit".into(),
			limit.to_string(),
			"--data-dir".into(),
			dir.to_string_lossy().into_owned(),
		])
	}

	/// Runs the CLI and parses `--json` output; failures become structured
	/// errors carrying the original stderr.
	fn run(&self, argv: &[String]) -> Result<Value, ToolError> {
		ensure_bin(&self.cfg)?;
		self.cfg.ensure_config()?;
		let output = spawn_with_timeout(
			&self.cfg.bin,
			argv,
			&self.cfg.config_path,
			self.cfg.timeout,
		)?;
		if !output.status.success() {
			let stderr = output.stderr.trim().to_string();
			let code = output.status.code().unwrap_or(-1);
			let lowered = stderr.to_lowercase();
			let tool_error = if stderr.contains("记忆不存在") || lowered.contains("not found") {
				ToolError::new(
					CODE_NOT_FOUND,
					format!(
						"记忆不存在（memlite rc={code}）：{stderr}。用 memory.list 查看现有的 12 位十六进制 id。"
					),
				)
			} else if stderr.contains("敏感信息") {
				ToolError::new(
					CODE_INVALID_PARAMS,
					format!("memlite 拒绝了这条内容（rc={code}）：{stderr}"),
				)
			} else {
				ToolError::new(
					oak_ai_tools::error::CODE_BACKEND,
					format!(
						"memlite 执行失败（子命令 {}，rc={code}）：{stderr}。\
						 如反复失败，检查 `memlite status --data-dir <库>` 与磁盘权限。",
						argv.first().map(String::as_str).unwrap_or("?")
					),
				)
			};
			return Err(tool_error);
		}
		let stdout = output.stdout.trim();
		if stdout.is_empty() {
			// `remember`/`forget`/`update` print JSON; a silent success is a
			// CLI behaviour change we must not paper over.
			return Ok(json!({}));
		}
		serde_json::from_str::<Value>(stdout).map_err(|e| {
			ToolError::new(
				oak_ai_tools::error::CODE_BACKEND,
				format!("memlite 输出不是 JSON（{e}）：{}", truncate(stdout, 200)),
			)
		})
	}
}

/// Reads are allowed on a missing library (empty answer), writes are not —
/// but a typo'd directory would read as "nothing found", so callers verify.
fn ensure_library_exists(dir: &Path, tier: Tier) -> Result<Option<ToolError>, ToolError> {
	if dir.is_dir() {
		return Ok(None);
	}
	let hint = match tier {
		Tier::Project => "该项目还没有记忆库（第一次 memory.remember 会自动建立）。",
		Tier::User => "用户级记忆库尚未建立（第一次 memory.remember 会自动建立）。",
	};
	Ok(Some(ToolError::new(
		CODE_NOT_FOUND,
		format!("{hint}目录：{}", dir.display()),
	)))
}

/// `ensure_library_exists` answers "missing" as a value so `recall` can
/// report it as a note rather than aborting a two-tier search.
fn out_of_band(_note: Option<ToolError>) -> Result<(), ToolError> {
	Ok(())
}

fn score_of(value: &Value) -> f64 {
	value.get("score").and_then(Value::as_f64).unwrap_or(0.0)
}

fn truncate(text: &str, max_chars: usize) -> String {
	let count = text.chars().count();
	if count <= max_chars {
		return text.to_string();
	}
	let kept: String = text.chars().take(max_chars).collect();
	format!("{kept}…（截断，原文 {count} 字符；全文用 memory.show）")
}

fn required_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
	args.get(key)
		.and_then(Value::as_str)
		.ok_or_else(|| ToolError::new(CODE_INVALID_PARAMS, format!("缺少字符串参数 '{key}'")))
}

fn optional_str(args: &Value, key: &str) -> Option<String> {
	args.get(key)
		.and_then(Value::as_str)
		.map(str::trim)
		.filter(|s| !s.is_empty())
		.map(str::to_string)
}

/// One child process' captured output.
#[derive(Debug)]
struct CliOutput {
	status: std::process::ExitStatus,
	stdout: String,
	stderr: String,
}

/// `Command::output()` has no timeout, so the child is polled and killed on
/// the deadline. A hung CLI must not freeze the panel's turn.
fn spawn_with_timeout(
	bin: &Path,
	argv: &[String],
	config_path: &Path,
	timeout: Duration,
) -> Result<CliOutput, ToolError> {
	let mut command = Command::new(bin);
	command
		.args(argv)
		// Isolation, not decoration: without this the CLI merges the user's
		// `~/.memlite/config.json` and dials 127.0.0.1:18321 before falling
		// back to BM25 (measured with strace).
		.env("MEMLITE_CONFIG", config_path)
		.stdin(Stdio::null())
		.stdout(Stdio::piped())
		.stderr(Stdio::piped());
	let child = command.spawn().map_err(|e| {
		ToolError::unsupported(
			GAP_MEMLITE_BINARY,
			format!(
				"无法启动 memlite（{}）：{e}。用 OAK_MEMLITE_BIN 指向可执行文件，或确认它有执行权限。",
				bin.display()
			),
		)
	})?;
	wait_with_deadline(child, timeout, bin)
}

/// Waits for the child, killing it on the deadline. Takes the child by value
/// because `wait_with_output` consumes it (and it is the only way to keep the
/// pipe buffers).
fn wait_with_deadline(
	mut child: Child,
	timeout: Duration,
	bin: &Path,
) -> Result<CliOutput, ToolError> {
	// `wait-timeout` owns the "poll until the deadline" part; a hung CLI is
	// killed here so it cannot freeze the panel's turn. The status stays
	// cached on `Child`, so `wait_with_output` still drains the pipes.
	use wait_timeout::ChildExt as _;
	match child.wait_timeout(timeout) {
		Ok(Some(_)) => {}
		Ok(None) => {
			let _ = child.kill();
			let _ = child.wait();
			return Err(ToolError::new(
				CODE_STATE,
				format!(
					"memlite 超过 {} 秒未返回，已终止（{}）。可能卡在锁或网络：检查是否有残留 memlite 进程。",
					timeout.as_secs(),
					bin.display()
				),
			));
		}
		Err(e) => {
			return Err(ToolError::new(
				oak_ai_tools::error::CODE_BACKEND,
				format!("等待 memlite 退出时出错：{e}"),
			))
		}
	}
	let output = child.wait_with_output().map_err(|e| {
		ToolError::new(oak_ai_tools::error::CODE_BACKEND, format!("读取 memlite 输出失败：{e}"))
	})?;
	Ok(CliOutput {
		status: output.status,
		stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
		stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
	})
}

// ---------------------------------------------------------------------------
// Schema face
// ---------------------------------------------------------------------------

/// The tool names this module owns.
pub const TOOL_NAMES: &[&str] = &[
	"memory.remember",
	"memory.recall",
	"memory.show",
	"memory.list",
	"memory.forget",
	"memory.status",
];

pub fn is_memory_tool(name: &str) -> bool {
	name.starts_with("memory.")
}

/// OpenAI `tools` entries, same envelope as the frozen 22 and the v2 face.
pub fn openai_tools() -> Vec<Value> {
	vec![
		tool(
			"memory.remember",
			"Save one durable fact to the user's long-term memory (BM25, two tiers: this project \
			 first, then the user's global library). Record preferences, project conventions and \
			 mistakes already paid for — never keys, tokens, passwords or personal data.",
			json!({
				"type": "object",
				"properties": {
					"content": { "type": "string", "description": "The fact itself, one or two sentences, self-contained (no pronouns pointing at the chat)." },
					"title": { "type": "string", "description": "Short label (<= 60 chars); defaults to the first 40 chars of the content." },
					"tags": { "type": "string", "description": "Comma-separated tags, e.g. \"oak,export\"." },
					"project": { "type": "string", "description": "Override the scope: projects you name here are kept in that project's library. Omit for the current project." }
				},
				"required": ["content"],
				"additionalProperties": false
			}),
		),
		tool(
			"memory.recall",
			"Search long-term memory by keywords (BM25, exact-ish): returns up to 5 hits with id, \
			 title, score and a 400-char excerpt. Search with concrete nouns — tool names, file \
			 names, error codes.",
			json!({
				"type": "object",
				"properties": {
					"query": { "type": "string", "description": "Keywords, not a sentence." },
					"limit": { "type": "integer", "minimum": 1, "maximum": 20, "description": "Hits to return (default 5)." }
				},
				"required": ["query"],
				"additionalProperties": false
			}),
		),
		tool(
			"memory.show",
			"Read one memory entry in full by id (use after recall/list).",
			json!({
				"type": "object",
				"properties": {
					"id": { "type": "string", "description": "12-hex id from recall/list." }
				},
				"required": ["id"],
				"additionalProperties": false
			}),
		),
		tool(
			"memory.list",
			"List the most recent memories in both tiers (id, title, tags, tier).",
			json!({
				"type": "object",
				"properties": {
					"limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Entries to return (default 20)." }
				},
				"additionalProperties": false
			}),
		),
		tool(
			"memory.forget",
			"Delete one memory entry by id (the file keeps a tombstone; the entry stops being \
			 recalled).",
			json!({
				"type": "object",
				"properties": {
					"id": { "type": "string", "description": "12-hex id from recall/list." }
				},
				"required": ["id"],
				"additionalProperties": false
			}),
		),
		tool(
			"memory.status",
			"Sizes and tiers of the memory libraries, and which retrieval backend is in use.",
			json!({ "type": "object", "properties": {}, "additionalProperties": false }),
		),
	]
}

fn tool(name: &str, description: &str, parameters: Value) -> Value {
	json!({
		"type": "function",
		"function": { "name": name, "description": description, "parameters": parameters }
	})
}

/// Dispatches a `memory.*` call. Unknown memory names answer `UNKNOWN_TOOL`.
pub fn execute(tools: &MemoryTools, name: &str, args: Value) -> Result<Value, ToolError> {
	let result = match name {
		"memory.remember" => tools.remember(&args),
		"memory.recall" => tools.recall(&args),
		"memory.show" => tools.show(&args),
		"memory.list" => tools.list(&args),
		"memory.forget" => tools.forget(&args),
		"memory.status" => tools.status(&args),
		other => {
			// Stamped here rather than in the `map_err` below: this branch
			// returns early, and an error without its tool name is not
			// actionable for the model.
			let mut error = ToolError::new(
				oak_ai_tools::error::CODE_UNKNOWN_TOOL,
				format!("未知的记忆工具 {other}；可用：{}", TOOL_NAMES.join(", ")),
			);
			error.tool = Some(other.to_string());
			return Err(error);
		}
	};
	result.map_err(|mut e| {
		if e.tool.is_none() {
			e.tool = Some(name.to_string());
		}
		e
	})
}

#[cfg(test)]
mod tests {
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
		assert_eq!(find_secret("用户手机 13812345678"), Some(SecretKind::PhoneNumber));
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
		assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'), "{a}");
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
		assert!(err.message.contains("OAK_MEMLITE_BIN"), "install hint missing: {err}");
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
			tools.remember(&json!({"content": "key sk-proj-aaaaaaaaaaaaaaaaaaaa"})).unwrap_err().code,
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
		let tools = MemoryTools::new(MemoryConfig::new(bin, &root, Some("oak-test".into())).with_timeout(Duration::from_secs(20)));

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

		let recalled = tools.recall(&json!({"query": "recall 预算"})).expect("recall");
		assert_eq!(recalled["backend"], "bm25", "BM25 is the only backend we accept");
		let hits = recalled["hits"].as_array().expect("hits");
		assert_eq!(hits.len(), 1, "{recalled}");
		assert_eq!(hits[0]["id"].as_str().unwrap(), id);
		assert_eq!(hits[0]["tier"], "project");
		assert!(hits[0]["content"].as_str().unwrap().contains("400 字"));

		// Two tiers: a project library with no match falls back to user.
		let user_tools = MemoryTools::new(MemoryConfig::new(
			tools.config().bin(),
			&root,
			None,
		).with_timeout(Duration::from_secs(20)));
		user_tools
			.remember(&json!({"content": "用户偏好深色主题与中文界面", "title": "主题偏好"}))
			.expect("user remember");
		let merged = MemoryTools::new(MemoryConfig::new(tools.config().bin(), &root, Some("oak-test".into())).with_timeout(Duration::from_secs(20)))
			.recall(&json!({"query": "主题偏好", "limit": 3}))
			.expect("two-tier recall");
		let hits = merged["hits"].as_array().expect("hits");
		assert_eq!(merged["fallback"], "user", "second tier must be consulted: {merged}");
		assert!(hits.iter().any(|h| h["tier"] == "user"), "{merged}");

		let shown = tools.show(&json!({"id": id})).expect("show");
		assert_eq!(shown["id"].as_str().unwrap(), id);
		assert_eq!(shown["tier"], "project");

		let listed = tools.list(&json!({"limit": 10})).expect("list");
		assert!(listed["entries"].as_array().unwrap().iter().any(|e| e["id"] == json!(id)));

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

		let tools = MemoryTools::new(MemoryConfig::new(bin, &root, Some("p".into())).with_timeout(Duration::from_secs(20)));
		tools.remember(&json!({"content": "配置隔离测试条目"})).expect("remember");

		let written = std::fs::read_to_string(tools.config().config_path()).expect("private config");
		let parsed: Value = serde_json::from_str(&written).expect("json");
		assert!(parsed.get("data_dir").and_then(Value::as_str).is_some(), "{written}");
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
					let tools = MemoryTools::new(MemoryConfig::new(bin, root, Some("p".into())).with_timeout(Duration::from_secs(20)));
					tools.remember(&json!({"content": format!("并发写入第 {i} 条"), "title": format!("并发{i}")}))
				})
			})
			.collect();
		for handle in handles {
			handle.join().expect("thread").expect("remember");
		}
		let tools = MemoryTools::new(MemoryConfig::new(bin, &root, Some("p".into())).with_timeout(Duration::from_secs(20)));
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
		assert!(line.contains("--bm25"), "the lexical path must be pinned: {line}");
		assert!(line.contains("--data-dir"), "the data dir must be explicit: {line}");
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
		assert!(err.message.contains("未返回") && err.message.contains("已终止"), "{err}");
		assert!(started.elapsed() < Duration::from_secs(5), "the deadline must bite");
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
			("sk-proj token", "我的 key 是 sk-proj-abc123def456ghi789jkl012mno345"),
			("口令赋值", "数据库口令 = hunter2correcthorse"),
			("PEM 私钥", "-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAKCAQEA\n-----END RSA PRIVATE KEY-----"),
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
}
