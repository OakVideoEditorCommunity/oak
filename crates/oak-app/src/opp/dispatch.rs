//! Request → [`HostServices`] dispatch (T7.2 P1.2).
//!
//! The P1 `oak-extensions` handler owns the protocol-level methods
//! (`events.*`, `session.log`, unknown → `METHOD_NOT_FOUND`); this layer
//! runs first for the six engine domains and falls through to it otherwise,
//! so a plugin's single request stream is served by both halves without
//! `oak-extensions` ever depending on this crate.

use oak_extensions::host_services::{
	AddEffectReq, AddMarkerReq, AddTransitionReq, AudioLevelsRequest, DeleteClipReq,
	ExportRequest, FrameRequest, HostError, HostServices, SetKeyframeReq, SetParamReq,
	SetWorkareaReq, ThumbnailsRequest,
};
use oak_extensions::protocol::{
	ErrorObject, Request, Response, INVALID_PARAMS, METHOD_NOT_FOUND,
};
use oak_extensions::session::{handle_host_request, HostSession};
use serde_json::{json, Value};

use super::host::OppHost;

/// The method prefixes this layer owns; everything else (including
/// `events.*` and `session.*`) goes to the P1 protocol handler.
const ENGINE_DOMAINS: [&str; 7] = [
	"project.",
	"media.",
	"timeline.",
	"node.",
	"render.",
	"export.",
	"edit.",
];

/// Serve one plugin request: engine domains → [`OppHost`], everything else
/// → the P1 session handler (which answers `METHOD_NOT_FOUND` for methods
/// neither half serves).
pub fn dispatch(host: &OppHost, session: &HostSession, req: &Request) -> Response {
	if ENGINE_DOMAINS.iter().any(|d| req.method.starts_with(d)) {
		return match execute(host, req) {
			Ok(value) => Response::success(req.id.clone(), value),
			Err(e) => Response {
				jsonrpc: "2.0".to_string(),
				id: req.id.clone(),
				result: None,
				error: Some(ErrorObject::from(e)),
			},
		};
	}
	handle_host_request(session, req)
}

/// Deserialize `params` into the protocol's own request struct.
fn params_as<T: serde::de::DeserializeOwned>(req: &Request) -> Result<T, HostError> {
	serde_json::from_value(req.params.clone()).map_err(|e| {
		HostError::with_data(
			INVALID_PARAMS,
			format!("invalid params for {}: {e}", req.method),
			json!({ "method": req.method }),
		)
	})
}

fn str_param<'a>(req: &'a Request, key: &str) -> Result<&'a str, HostError> {
	req.params
		.get(key)
		.and_then(Value::as_str)
		.filter(|s| !s.is_empty())
		.ok_or_else(|| {
			HostError::with_data(
				INVALID_PARAMS,
				format!("{} requires a string {key}", req.method),
				json!({ "method": req.method, "field": key }),
			)
		})
}

/// The transaction token carried alongside a confirmation-class call.
fn txn_param(req: &Request) -> Option<String> {
	req.params
		.get("txn")
		.and_then(Value::as_str)
		.map(str::to_string)
}

fn to_value<T: serde::Serialize>(v: T) -> Result<Value, HostError> {
	serde_json::to_value(v).map_err(|e| HostError::internal(format!("encode: {e}")))
}

fn execute(host: &OppHost, req: &Request) -> Result<Value, HostError> {
	match req.method.as_str() {
		// ---- project.* ----
		"project.get_info" => to_value(host.project_get_info()?),
		"project.open" => to_value(host.project_open(str_param(req, "path")?, txn_param(req).as_deref())?),
		"project.save" => {
			let path = req.params.get("path").and_then(Value::as_str);
			to_value(host.project_save(path, txn_param(req).as_deref())?)
		}

		// ---- media.* ----
		"media.probe" => to_value(host.media_probe(str_param(req, "source")?)?),
		"media.list_footage" => to_value(host.media_list_footage()?),
		"media.import_footage" => {
			to_value(host.media_import_footage(str_param(req, "path")?, txn_param(req).as_deref())?)
		}

		// ---- timeline.* ----
		"timeline.get_structure" => to_value(host.timeline_get_structure()?),
		"timeline.add_track" => to_value(host.timeline_add_track(
			params_as(req)?,
			txn_param(req).as_deref(),
		)?),
		"timeline.place_clip" => to_value(host.timeline_place_clip(
			params_as(req)?,
			txn_param(req).as_deref(),
		)?),
		"timeline.split_clip" => to_value(host.timeline_split_clip(
			params_as(req)?,
			txn_param(req).as_deref(),
		)?),
		"timeline.trim_clip" => to_value(host.timeline_trim_clip(
			params_as(req)?,
			txn_param(req).as_deref(),
		)?),
		"timeline.move_clip" => to_value(host.timeline_move_clip(
			params_as(req)?,
			txn_param(req).as_deref(),
		)?),
		"timeline.ripple_delete" => to_value(host.timeline_ripple_delete(
			params_as(req)?,
			txn_param(req).as_deref(),
		)?),
		"timeline.delete_clip" => to_value(host.timeline_delete_clip(
			params_as::<DeleteClipReq>(req)?,
			txn_param(req).as_deref(),
		)?),
		"timeline.add_transition" => to_value(host.timeline_add_transition(
			params_as::<AddTransitionReq>(req)?,
			txn_param(req).as_deref(),
		)?),
		"timeline.add_marker" => to_value(host.timeline_add_marker(
			params_as::<AddMarkerReq>(req)?,
			txn_param(req).as_deref(),
		)?),
		"timeline.set_workarea" => to_value(host.timeline_set_workarea(
			params_as::<SetWorkareaReq>(req)?,
			txn_param(req).as_deref(),
		)?),

		// ---- node.* ----
		"node.list_types" => to_value(host.node_list_types()?),
		"node.get_params" => to_value(host.node_get_params(&str_param(req, "node")?.to_string())?),
		"node.add_effect" => to_value(host.node_add_effect(
			params_as::<AddEffectReq>(req)?,
			txn_param(req).as_deref(),
		)?),
		"node.set_param" => to_value(host.node_set_param(
			params_as::<SetParamReq>(req)?,
			txn_param(req).as_deref(),
		)?),
		"node.set_keyframe" => to_value(host.node_set_keyframe(
			params_as::<SetKeyframeReq>(req)?,
			txn_param(req).as_deref(),
		)?),
		"node.remove" => to_value(host.node_remove(
			&str_param(req, "node")?.to_string(),
			txn_param(req).as_deref(),
		)?),

		// ---- render.* ----
		"render.get_frame" => to_value(host.render_get_frame(params_as::<FrameRequest>(req)?)?),
		"render.get_thumbnails" => to_value(host.render_get_thumbnails(params_as::<ThumbnailsRequest>(
			req,
		)?)?),
		"render.get_audio_levels" => to_value(host.render_get_audio_levels(
			params_as::<AudioLevelsRequest>(req)?,
		)?),

		// ---- export.* ----
		"export.start" => to_value(host.export_start(
			params_as::<ExportRequest>(req)?,
			txn_param(req).as_deref(),
		)?),
		"export.cancel" => {
			host.export_cancel(&str_param(req, "job")?.to_string())?;
			Ok(Value::Null)
		}

		// ---- edit.* (undo domain) ----
		"edit.begin" => to_value(host.edit_begin(str_param(req, "label")?)?),
		"edit.commit" => {
			host.edit_commit(str_param(req, "txn")?)?;
			Ok(Value::Null)
		}
		"edit.abort" => {
			host.edit_abort(str_param(req, "txn")?)?;
			Ok(Value::Null)
		}
		"edit.undo" => Ok(json!(host.edit_undo()?)),
		"edit.redo" => Ok(json!(host.edit_redo()?)),

		// A known domain with a method this crate does not serve at all.
		other => Err(HostError::with_data(
			METHOD_NOT_FOUND,
			format!("unknown method {other}"),
			json!({ "method": other, "unsupported": true }),
		)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Wire-layer coverage: an engine-domain request reaches the host (and
	/// inherits its structured `Unsupported`), while everything outside the
	/// seven domains falls through to the P1 protocol handler.
	#[test]
	fn dispatch_routes_domains_and_falls_through_to_the_p1_handler() {
		let host = OppHost::new();
		let session = HostSession::new();

		// In-domain but unwired → structured Unsupported from the host.
		let req = Request::new(1u64, "media.probe", json!({ "source": "/x.mp4" }));
		let resp = dispatch(&host, &session, &req);
		let err = resp.error.expect("media.probe must not succeed");
		assert_eq!(err.code, METHOD_NOT_FOUND);
		let data = err.data.expect("Unsupported must carry structured data");
		assert_eq!(data["unsupported"], true);
		assert_eq!(data["method"], "media.probe");

		// Outside the engine domains → the P1 session handler owns it.
		let req2 = Request::new(
			2u64,
			"session.log",
			json!({ "level": "info", "message": "hi" }),
		);
		let resp2 = dispatch(&host, &session, &req2);
		assert!(resp2.is_success(), "{resp2:?}");
		assert_eq!(session.logs().len(), 1, "P1 handler must record the log line");

		// A method in no domain at all → the P1 handler's METHOD_NOT_FOUND.
		let req3 = Request::new(3u64, "ui.set_tree", json!({}));
		let resp3 = dispatch(&host, &session, &req3);
		assert_eq!(
			resp3.error.expect("unknown method must fail").code,
			METHOD_NOT_FOUND
		);
	}
}
