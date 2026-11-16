use super::*;

/// Every shipped pack (the YAML files in `assets/i18n/`) parses and
/// covers exactly the en-US key set, and carries its `language.name`
/// display name. User packs from `$HOME/.oak/i18n` are deliberately
/// not held to the key set (a partial override pack is legitimate).
#[test]
fn every_shipped_pack_matches_en_keys() {
	let tables = tables().read().unwrap_or_else(|e| e.into_inner());
	let en = tables.get("en-US").expect("en-US pack");
	assert!(!en.is_empty(), "en-US pack parses to a non-empty table");
	for entry in std::fs::read_dir(
		std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/i18n"),
	)
	.expect("dev checkout packs")
	{
		let path = entry.unwrap().path();
		if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
			continue;
		}
		let code = path.file_stem().unwrap().to_str().unwrap().to_string();
		let table = tables.get(&code).unwrap_or_else(|| panic!("{code} pack"));
		assert!(!table.is_empty(), "{code} pack parses to a non-empty table");
		assert!(
			table.contains_key("language.name"),
			"{code} pack has no language.name display name"
		);
		for key in en.keys() {
			assert!(
				table.contains_key(key),
				"key {key} missing from the {code} pack"
			);
		}
		for key in table.keys() {
			assert!(
				en.contains_key(key),
				"key {key} missing from the en-US pack"
			);
		}
	}
}

/// No value is left as the raw key, and the two languages differ
/// except for an explicit shared list (brand names, units).
#[test]
fn no_untranslated_values() {
	let shared = [
		"Olive Dark",
		"Olive Light",
		"English",
		"简体中文",
		"en-US",
		"zh-CN",
		"OpenGL",
		"Metal",
		"Vulkan",
		"MP4",
		"OpenFX",
		"RGB",
		"HSV",
		"1/2",
		"1/4",
		"1/8",
		// Standard colorimetry names — identical across languages.
		"sRGB / Rec.709",
		"Display P3",
		"Rec.2020",
		"sRGB",
		"Gamma 2.2",
		"PQ (ST 2084)",
		"HLG",
	];
	let tables = tables().read().unwrap_or_else(|e| e.into_inner());
	let en = tables.get("en-US").unwrap();
	let zh = tables.get("zh-CN").unwrap();
	for (key, en_value) in en {
		assert_ne!(
			*en_value,
			key.as_str(),
			"en-US value for {key} is the raw key"
		);
		let zh_value = zh.get(key).unwrap();
		assert_ne!(
			*zh_value,
			key.as_str(),
			"zh-CN value for {key} is the raw key"
		);
		if en_value == zh_value && !shared.contains(en_value) {
			panic!("key {key} has identical en-US and zh-CN values ({en_value:?})");
		}
	}
}

/// Every built-in node type the factory registers carries a
/// `node.<type-id suffix>.name` entry in the reference pack *and* in
/// the translation: the effect library and the inspector build that key
/// at runtime, so a node without an entry would show its English name
/// in every language — and one whose zh value repeats the English name
/// was never translated at all. The matching parameter keys are held by
/// `effectchain`'s own test against the live parameter list.
#[test]
fn every_builtin_node_name_is_translated() {
	let tables = tables().read().unwrap_or_else(|e| e.into_inner());
	let en = tables.get("en-US").expect("en-US pack");
	let zh = tables.get("zh-CN").expect("zh-CN pack");
	let mut nodes = 0usize;
	for meta in quercus_engine_rs::effect::factory_entries() {
		let suffix = meta.type_id.rsplit('.').next().unwrap_or(&meta.type_id);
		let key = format!("node.{suffix}.name");
		let en_value = en
			.get(&key)
			.unwrap_or_else(|| panic!("{key} missing from the en-US pack"));
		assert_eq!(
			*en_value, meta.name,
			"{key} does not carry the node's English name"
		);
		let zh_value = zh
			.get(&key)
			.unwrap_or_else(|| panic!("{key} missing from the zh-CN pack"));
		assert_ne!(zh_value, en_value, "{key} is not translated ({en_value:?})");
		nodes += 1;
	}
	assert!(
		nodes >= 60,
		"the factory registers the built-in node types ({nodes})"
	);
}

/// `tr` falls back from the active language to English and then to
/// the key itself.
#[test]
fn tr_falls_back_to_english_then_the_key() {
	let _guard = crate::i18n::test_lock();
	set_language_code("zh-CN");
	assert_eq!(tr("menu.file"), "文件(F)");
	set_language_code("en-US");
	assert_eq!(tr("menu.file"), "File(F)");
	assert_eq!(tr("no.such.key"), "no.such.key");
}

/// A custom pack on disk overrides built-in values and registers a
/// new language. The active language is pinned to en-US: the user
/// pack overrides the built-in en-US table, so the assertions below
/// must not inherit another test's active language.
#[test]
fn user_pack_overrides_and_extends() {
	let _guard = crate::i18n::test_lock();
	set_language_code("en-US");
	let dir = std::env::temp_dir().join(format!("oak_i18n_pack_{}", std::process::id()));
	let home = dir.join("home");
	let pack_dir = home.join(".oak/i18n");
	std::fs::create_dir_all(&pack_dir).unwrap();
	std::fs::write(pack_dir.join("pirate.yaml"), "\"menu.file\": \"Arrr\"\n").unwrap();
	std::fs::write(
		pack_dir.join("en-US.yaml"),
		"\"menu.file\": \"Yarr File\"\n",
	)
	.unwrap();
	unsafe { std::env::set_var("HOME", &home) };
	reload_for_test();
	assert_eq!(
		tr("menu.file"),
		"Yarr File",
		"the user pack overrides the built-in"
	);
	assert!(available_languages().iter().any(|c| c == "pirate"));
	set_language_code("pirate");
	assert_eq!(tr("menu.file"), "Arrr");
	set_language_code("zh-CN");
	unsafe { std::env::remove_var("HOME") };
	reload_for_test();
	let _ = std::fs::remove_dir_all(&dir);
	// Leave the process language at the default for the next test.
	set_language_code("en-US");
}
