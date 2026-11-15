//! Text-form tool-call fallback (T9.4).
//!
//! Some models (Qwen/Llama-family, notably) emit a tool call as **text**
//! instead of using the function-calling channel:
//!
//! ```text
//! <tool_call>
//! <function=sequence.set_format>
//! <parameter=sequence>3
//! <parameter=fps>60</tool_call>
//! ```
//!
//! The provider then returns an empty `tool_calls` array and the markup in
//! `content`, which used to end the turn with the XML shown to the user as if
//! it were an answer (field report, 2026-11-11). [`scan`] recognises that
//! shape so the loop can execute it through the very same dispatch path.
//!
//! Scope is deliberately narrow (首辅 ruling, T9.4 C): only the three forms
//! measured in the field — `<tool_call>`, `<function=NAME>`,
//! `<parameter=KEY>VALUE`. Variants such as `<parameter name=…>` are **not**
//! guessed; they land in [`TextCallScan::unparsed`] so we can see what a model
//! actually sent instead of inventing a grammar. (TODO: extend only with
//! captured samples, never by speculation.)

use serde_json::{Map, Value};

use super::llm::ToolCall;

/// How many calls one response may carry before the whole block is treated
/// as malformed (防病态输入把一轮塞爆, 首辅 ruling B).
pub const MAX_TEXT_CALLS: usize = 8;

/// The result of scanning one assistant message.
#[derive(Debug, Clone, PartialEq)]
pub struct TextCallScan {
	/// Parsed calls, in the order they appeared.
	pub calls: Vec<ToolCall>,
	/// What is left after the markup is removed: the prose that may safely
	/// be sent back in the conversation history (never the raw XML — feeding
	/// it back teaches the model to keep using it).
	pub prose: String,
	/// Set when the content looked like a text-form call but no call could
	/// be parsed: the offending raw slice, for `detail` and debugging.
	pub unparsed: Option<String>,
}

impl TextCallScan {
	/// `true` when the content carried any text-form marker at all.
	pub fn saw_markup(&self) -> bool {
		!self.calls.is_empty() || self.unparsed.is_some()
	}
}

const OPEN: &str = "<tool_call";
const CLOSE: &str = "</tool_call";
const FUNCTION: &str = "<function=";
const PARAMETER: &str = "<parameter=";

/// Scans `content` for text-form tool calls.
pub fn scan(content: &str) -> TextCallScan {
	let mut calls: Vec<ToolCall> = Vec::new();
	let mut prose = String::with_capacity(content.len());
	let mut unparsed: Option<String> = None;
	let mut rest = content;

	while let Some(start) = rest.find(OPEN) {
		// Everything before the block is prose.
		prose.push_str(&rest[..start]);
		let after_open = &rest[start..];
		// A block ends at `</tool_call` or at the end of the input (a missing
		// close tag is the *field* case, not an error).
		let (block, tail) = match after_open.find(CLOSE) {
			Some(end) => {
				let close_end = end + CLOSE.len();
				// Swallow a trailing `>` if present so it never leaks to prose.
				let close_end = if after_open[close_end..].starts_with('>') {
					close_end + 1
				} else {
					close_end
				};
				(&after_open[..close_end], &after_open[close_end..])
			}
			None => (after_open, ""),
		};

		match parse_block(block) {
			Ok(call) => calls.push(call),
			Err(reason) => {
				let raw = block.trim().to_string();
				unparsed = Some(match unparsed {
					Some(prev) => format!("{prev}\n{reason}: {raw}"),
					None => format!("{reason}: {raw}"),
				});
			}
		}
		rest = tail;
	}
	prose.push_str(rest);

	// The cap turns a pathological response into "malformed" rather than an
	// eight-call burst inside one round.
	if calls.len() > MAX_TEXT_CALLS {
		let raw = content.trim().to_string();
		unparsed = Some(format!(
			"more than {MAX_TEXT_CALLS} text-form calls in one response: {raw}"
		));
		calls.clear();
	}

	// Markup without a block: `content` carried `<function=`/`<parameter=`
	// but never opened a `<tool_call>`. That is still a model trying to call a
	// tool, so it must be reported — never shown as an answer.
	if calls.is_empty() && unparsed.is_none() && (content.contains(FUNCTION) || content.contains(PARAMETER)) {
		unparsed = Some(format!(
			"text-form markup without a `<tool_call>` block: {}",
			content.trim()
		));
	}

	let prose = prose.trim().to_string();
	TextCallScan {
		calls,
		prose,
		unparsed,
	}
}

/// Parses one `<tool_call>…</tool_call>` block (either end may be missing).
fn parse_block(block: &str) -> Result<ToolCall, String> {
	let Some(fn_at) = block.find(FUNCTION) else {
		return Err("no `<function=` in the block".to_string());
	};
	let after_fn = &block[fn_at + FUNCTION.len()..];
	let name_end = after_fn
		.find(|c: char| c == '>' || c.is_whitespace())
		.unwrap_or(after_fn.len());
	let name = after_fn[..name_end].trim();
	if name.is_empty() {
		return Err("empty `<function=>` name".to_string());
	}
	// A second `<function=` inside one block is a shape we have never seen:
	// refuse it instead of guessing which call owns the parameters.
	let body = &after_fn[name_end..];
	if body.contains(FUNCTION) {
		return Err("more than one `<function=` in one block".to_string());
	}

	let mut args = Map::new();
	let mut cursor = body;
	while let Some(at) = cursor.find(PARAMETER) {
		let after_param = &cursor[at + PARAMETER.len()..];
		let key_end = after_param
			.find(|c: char| c == '>' || c.is_whitespace())
			.unwrap_or(after_param.len());
		let key = after_param[..key_end].trim().to_string();
		if key.is_empty() {
			return Err("empty `<parameter=>` name".to_string());
		}
		// The value starts after the key's `>` (when present) and runs until
		// the next markup token — this is what tolerates the field sample,
		// where `fps`'s value is terminated by `</tool_call` instead of
		// `</parameter>`.
		let value_start = match after_param[key_end..].find('>') {
			Some(gt) => key_end + gt + 1,
			None => key_end,
		};
		let value_region = &after_param[value_start..];
		let cut = [
			PARAMETER,
			FUNCTION,
			CLOSE,
			"</parameter>",
			"</parameter",
		]
		.iter()
		.filter_map(|token| value_region.find(token))
		.min()
		.unwrap_or(value_region.len());
		let raw_value = value_region[..cut].trim();
		args.insert(key, infer_value(raw_value));
		cursor = &value_region[cut..];
	}

	if args.is_empty() {
		return Err(format!("`{name}` has no `<parameter=`"));
	}
	Ok(ToolCall {
		id: String::new(), // filled in by the caller with a round-unique id
		name: name.to_string(),
		arguments: Value::Object(args),
	})
}

/// Value inference: only the unambiguous literal forms.
///
/// Deliberately conservative (首辅 ruling): a quoted `"60"` stays a string —
/// the v2 parsers already tolerate numeric strings, and coercing here would
/// hide what the model actually sent. No expressions, no unclosed JSON.
pub fn infer_value(raw: &str) -> Value {
	let text = raw.trim();
	match text {
		"true" => return Value::Bool(true),
		"false" => return Value::Bool(false),
		"null" => return Value::Null,
		_ => {}
	}
	if is_integer(text) {
		if let Ok(n) = text.parse::<i64>() {
			return Value::from(n);
		}
	}
	if is_decimal(text) {
		if let Ok(f) = text.parse::<f64>() {
			if let Some(n) = serde_json::Number::from_f64(f) {
				return Value::Number(n);
			}
		}
	}
	if (text.starts_with('{') && text.ends_with('}'))
		|| (text.starts_with('[') && text.ends_with(']'))
	{
		if let Ok(value) = serde_json::from_str::<Value>(text) {
			return value;
		}
	}
	Value::String(text.to_string())
}

fn is_integer(text: &str) -> bool {
	let digits = text.strip_prefix('-').unwrap_or(text);
	!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

fn is_decimal(text: &str) -> bool {
	let Some((int_part, frac)) = text.split_once('.') else {
		return false;
	};
	let int_ok = {
		let d = int_part.strip_prefix('-').unwrap_or(int_part);
		!d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())
	};
	int_ok && !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	/// **The field sample, verbatim** (陛下, 2026-11-11 20:53). Note the
	/// missing `</parameter>` before `</tool_call`: the value of `fps` is
	/// terminated by the block close.
	const FIELD_SAMPLE: &str = "<tool_call>\n<function=sequence.set_format>\n<parameter=sequence>3\n<parameter=fps>60</tool_call>";

	#[test]
	fn the_field_sample_parses_verbatim() {
		let scan = scan(FIELD_SAMPLE);
		assert_eq!(scan.calls.len(), 1, "one call: {scan:?}");
		let call = &scan.calls[0];
		assert_eq!(call.name, "sequence.set_format");
		assert_eq!(
			call.arguments,
			json!({ "sequence": 3, "fps": 60 }),
			"`sequence` is an integer and `fps` a number: {}",
			call.arguments
		);
		assert_eq!(scan.prose, "", "no prose in the field sample");
		assert!(scan.unparsed.is_none());
	}

	#[test]
	fn several_blocks_keep_their_order() {
		let content = concat!(
			"<tool_call><function=timeline.add_track><parameter=type>video</tool_call>\n",
			"<tool_call><function=project.get_overview></function></tool_call>"
		);
		// The second block has no parameter: that is malformed by our rules,
		// so it must surface rather than be silently dropped.
		let scan = scan(content);
		assert_eq!(scan.calls.len(), 1, "{scan:?}");
		assert_eq!(scan.calls[0].name, "timeline.add_track");
		assert_eq!(scan.calls[0].arguments, json!({ "type": "video" }));
		assert!(scan.unparsed.is_some(), "the parameterless block is reported");
	}

	#[test]
	fn missing_close_tags_and_crlf_are_tolerated() {
		let content = "<tool_call>\r\n<function=sequence.set_format>\r\n<parameter=fps>29.97\r\n";
		let scan = scan(content);
		assert_eq!(scan.calls.len(), 1, "{scan:?}");
		assert_eq!(scan.calls[0].arguments, json!({ "fps": 29.97 }));
		assert_eq!(scan.prose, "");
	}

	#[test]
	fn prose_around_the_block_is_kept_and_the_markup_is_not() {
		let content = "Sure, setting the frame rate.\n<tool_call>\n<function=sequence.set_format>\n<parameter=fps>24\n</tool_call>\nDone.";
		let scan = scan(content);
		assert_eq!(scan.calls.len(), 1);
		assert!(scan.prose.contains("setting the frame rate"), "{}", scan.prose);
		assert!(scan.prose.contains("Done."), "{}", scan.prose);
		assert!(!scan.prose.contains('<'), "markup leaked into prose: {}", scan.prose);
	}

	#[test]
	fn malformed_markup_never_becomes_a_call() {
		for content in [
			"<tool_call>\n<parameter=fps>60\n</tool_call>", // no function
			"<function=sequence.set_format>\n<parameter=fps>60", // no block
			"<tool_call><function=></tool_call>",          // empty name
			"<tool_call><function=sequence.set_format></tool_call>", // no params
		] {
			let scan = scan(content);
			assert!(scan.calls.is_empty(), "must not parse: {content}");
			assert!(scan.unparsed.is_some(), "must be reported: {content}");
		}
	}

	/// The negative that matters most: ordinary prose that merely contains
	/// `<` must never look like a tool call.
	#[test]
	fn ordinary_prose_is_not_mistaken_for_a_call() {
		for content in [
			"如果 width < 100，就裁掉边缘。",
			"Use `a < b` when comparing; nothing else.",
			"",
			"Plain answer with no markup at all.",
		] {
			let scan = scan(content);
			assert!(scan.calls.is_empty(), "false positive: {content}");
			assert!(
				scan.unparsed.is_none(),
				"`<` alone is not a text-form call: {content}"
			);
			assert!(!scan.saw_markup(), "{content}");
		}
	}

	#[test]
	fn a_pathological_burst_is_refused_rather_than_executed() {
		let one = "<tool_call><function=sequence.set_format><parameter=fps>60</tool_call>";
		let content = one.repeat(MAX_TEXT_CALLS + 1);
		let scan = scan(&content);
		assert!(scan.calls.is_empty(), "over the cap: no calls");
		assert!(scan.unparsed.is_some(), "over the cap: reported");
	}

	#[test]
	fn value_inference_is_conservative() {
		assert_eq!(infer_value("true"), json!(true));
		assert_eq!(infer_value("false"), json!(false));
		assert_eq!(infer_value("null"), Value::Null);
		assert_eq!(infer_value("60"), json!(60));
		assert_eq!(infer_value("-12"), json!(-12));
		assert_eq!(infer_value("29.97"), json!(29.97));
		assert_eq!(infer_value("{\"a\":1}"), json!({"a": 1}));
		assert_eq!(infer_value("[1,2]"), json!([1, 2]));
		// Strings stay strings: the v2 parsers already read numeric strings,
		// and coercing here would hide what the model actually sent.
		assert_eq!(infer_value("\"60\""), json!("\"60\""));
		assert_eq!(infer_value("六十"), json!("六十"));
		assert_eq!(infer_value(" 60 "), json!(60));
		// Unclosed JSON is not guessed.
		assert_eq!(infer_value("{\"a\":1"), json!("{\"a\":1"));
	}
}
