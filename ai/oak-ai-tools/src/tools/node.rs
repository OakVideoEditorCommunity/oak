//! Arguments and parameter-range policy for `node.*`.
//!
//! The bounded-key table mirrors method-map.md table 3 (ranges
//! backfilled from `crates/oak-node/src/nodes/*.rs` at the T0 freeze):
//! keys whose declared range is unique across node types are bounded,
//! keys whose range differs per node type stay lower-bounded or fully
//! open and must be read back with `node.list_types` first.

use serde_json::Value;

use crate::error::ToolError;
use crate::tools::common::{opt_str, req_str};

/// `node.add_effect`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddEffectArgs {
	/// Clip receiving the effect.
	pub clip: String,
	/// Node type id or OFX identifier (open set).
	pub effect: String,
	/// Insert position (`None` = append).
	pub index: Option<usize>,
}

/// Parses `node.add_effect`.
pub fn parse_add_effect(args: &Value) -> Result<AddEffectArgs, ToolError> {
	Ok(AddEffectArgs {
		clip: req_str(args, "clip")?.to_string(),
		effect: req_str(args, "effect")?.to_string(),
		index: match args.get("index") {
			None | Some(Value::Null) => None,
			Some(_) => Some(
				args.get("index")
					.and_then(Value::as_u64)
					.ok_or_else(|| ToolError::invalid_params("field 'index' must be an integer >= 0"))?
					 as usize,
			),
		},
	})
}

/// `node.set_param`
#[derive(Debug, Clone, PartialEq)]
pub struct SetParamArgs {
	/// Effect node instance id.
	pub node: String,
	/// Parameter key.
	pub key: String,
	/// New value (number / bool / string / `[x, y]`).
	pub value: Value,
}

/// Parses `node.set_param`.
pub fn parse_set_param(args: &Value) -> Result<SetParamArgs, ToolError> {
	let value = args
		.get("value")
		.cloned()
		.ok_or_else(|| ToolError::invalid_params("missing field 'value'"))?;
	Ok(SetParamArgs {
		node: req_str(args, "node")?.to_string(),
		key: req_str(args, "key")?.to_string(),
		value,
	})
}

/// `node.list_types`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListTypesArgs {
	/// `effect` / `transition` / `all`.
	pub category: Option<String>,
	/// Optional instance id: also return that node's live parameters.
	pub node: Option<String>,
}

/// Parses `node.list_types`.
pub fn parse_list_types(args: &Value) -> Result<ListTypesArgs, ToolError> {
	let category = opt_str(args, "category")?.map(str::to_string);
	if let Some(category) = &category {
		if !matches!(category.as_str(), "effect" | "transition" | "all") {
			return Err(ToolError::invalid_params(format!(
				"category must be effect|transition|all, got '{category}'"
			)));
		}
	}
	Ok(ListTypesArgs {
		category,
		node: opt_str(args, "node")?.map(str::to_string),
	})
}

/// A parameter range: `(lower, upper)`, `None` = open on that side.
pub type ParamRange = (Option<f64>, Option<f64>);

/// The frozen range table (method-map table 3).
///
/// * bounded keys → `(Some(min), Some(max))`
/// * lower-bounded keys → `(Some(0.0), None)`
/// * everything else → not present = explicitly open (never guessed)
pub fn range_for(key: &str) -> Option<ParamRange> {
	Some(match key {
		"temperature_in" => (Some(1000.0), Some(40000.0)),
		"tint_in" => (Some(-1.0), Some(1.0)),
		"saturation_in" => (Some(0.0), Some(4.0)),
		"mix_in" => (Some(0.0), Some(1.0)),
		"left_in" | "top_in" | "right_in" | "bottom_in" => (Some(0.0), Some(1.0)),
		"opacity_in" | "radius_in" | "volume_in" | "threshold_in" | "strength_in"
		| "feather_in" | "lower_tolerance_in" => (Some(0.0), None),
		// Explicitly open (method-map table 3): gamma_in, amount_in,
		// upper_tolerance_in, method_in, color_in and every other key.
		_ => return None,
	})
}

/// Enforces [`range_for`] on a numeric value (the schema already does
/// this for the bounded keys; the backends repeat it so a direct
/// backend call is equally safe).
pub fn check_range(key: &str, value: &Value) -> Result<(), ToolError> {
	let Some(number) = value.as_f64() else {
		return Ok(()); // non-numeric values are gated by the schema
	};
	let Some((low, high)) = range_for(key) else {
		return Ok(());
	};
	if let Some(low) = low {
		if number < low {
			return Err(ToolError::invalid_params(format!(
				"parameter '{key}' = {number} is below the minimum {low}"
			)));
		}
	}
	if let Some(high) = high {
		if number > high {
			return Err(ToolError::invalid_params(format!(
				"parameter '{key}' = {number} is above the maximum {high}"
			)));
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	#[test]
	fn range_table_matches_the_freeze() {
		assert_eq!(range_for("temperature_in"), Some((Some(1000.0), Some(40000.0))));
		assert_eq!(range_for("tint_in"), Some((Some(-1.0), Some(1.0))));
		assert_eq!(range_for("opacity_in"), Some((Some(0.0), None)));
		assert_eq!(range_for("gamma_in"), None); // explicitly open
		assert_eq!(range_for("amount_in"), None); // explicitly open
		assert_eq!(range_for("whatever_else"), None);
	}

	#[test]
	fn range_checking() {
		assert!(check_range("saturation_in", &json!(2.0)).is_ok());
		assert!(check_range("saturation_in", &json!(-0.5)).is_err());
		assert!(check_range("temperature_in", &json!(50000.0)).is_err());
		assert!(check_range("opacity_in", &json!(-1.0)).is_err());
		assert!(check_range("opacity_in", &json!(true)).is_ok());
		assert!(check_range("gamma_in", &json!(-99.0)).is_ok()); // open key
	}

	#[test]
	fn arg_parsing() {
		let args = json!({"clip": "clip-1", "effect": "org.olivevideoeditor.Olive.blur", "index": 2});
		let parsed = parse_add_effect(&args).unwrap();
		assert_eq!(parsed.index, Some(2));
		let set = json!({"node": "node-1", "key": "radius_in", "value": 4.0});
		let parsed = parse_set_param(&set).unwrap();
		assert_eq!(parsed.value, json!(4.0));
	}
}
