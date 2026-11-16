use super::*;
use oak_ai_tools::mock::MockBackend;

fn server() -> Server {
	Server::new(Box::new(MockBackend::new()), "mock")
}

fn init(server: &mut Server) -> Value {
	server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 1,
			"method": "initialize",
			"params": {
				"protocolVersion": "2024-11-05",
				"capabilities": {},
				"clientInfo": { "name": "unit-test", "version": "0" },
			},
		}))
		.expect("initialize is a request")
}

#[test]
fn initialize_negotiates_and_advertises_tools() {
	let mut server = server();
	let response = init(&mut server);
	assert_eq!(response["result"]["protocolVersion"], "2024-11-05");
	assert_eq!(response["result"]["serverInfo"]["name"], SERVER_NAME);
	assert_eq!(
		response["result"]["capabilities"]["tools"]["listChanged"],
		false
	);
	assert_eq!(server.protocol_version(), Some("2024-11-05"));
}

#[test]
fn unknown_protocol_version_falls_back_to_the_baseline() {
	let mut server = server();
	let response = server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 2,
			"method": "initialize",
			"params": { "protocolVersion": "1999-01-01" },
		}))
		.unwrap();
	assert_eq!(
		response["result"]["protocolVersion"],
		BASELINE_PROTOCOL_VERSION
	);
}

#[test]
fn requests_before_initialize_are_rejected() {
	let mut server = server();
	let response = server
		.handle(&json!({"jsonrpc": "2.0", "id": 7, "method": "tools/list"}))
		.unwrap();
	assert_eq!(response["error"]["code"], rpc::SERVER_NOT_INITIALIZED);
	assert_eq!(response["id"], 7);
}

#[test]
fn initialized_notification_has_no_response_and_keeps_the_session() {
	let mut server = server();
	init(&mut server);
	let response = server.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
	assert!(response.is_none(), "notifications must not be answered");
	let response = server
		.handle(&json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}}))
		.unwrap();
	assert_eq!(response["result"]["tools"].as_array().unwrap().len(), 22);
}

#[test]
fn ping_is_answered() {
	let mut server = server();
	init(&mut server);
	let response = server
		.handle(&json!({"jsonrpc": "2.0", "id": 4, "method": "ping"}))
		.unwrap();
	assert_eq!(response["result"], json!({}));
}

#[test]
fn unknown_method_is_method_not_found() {
	let mut server = server();
	init(&mut server);
	let response = server
		.handle(&json!({"jsonrpc": "2.0", "id": 5, "method": "tools/teleport"}))
		.unwrap();
	assert_eq!(response["error"]["code"], rpc::METHOD_NOT_FOUND);
}

#[test]
fn wrong_jsonrpc_member_is_invalid_request() {
	let mut server = server();
	init(&mut server);
	let response = server
		.handle(&json!({"jsonrpc": "1.0", "id": 6, "method": "tools/list"}))
		.unwrap();
	assert_eq!(response["error"]["code"], rpc::INVALID_REQUEST);
}

#[test]
fn batch_frames_are_rejected() {
	let mut server = server();
	init(&mut server);
	let response = server.handle(&json!([])).unwrap();
	assert_eq!(response["error"]["code"], rpc::INVALID_REQUEST);
}

#[test]
fn tools_call_requires_a_name_and_object_arguments() {
	let mut server = server();
	init(&mut server);
	let response = server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 8,
			"method": "tools/call",
			"params": { "arguments": {} },
		}))
		.unwrap();
	assert_eq!(response["error"]["code"], rpc::INVALID_PARAMS);

	let response = server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 9,
			"method": "tools/call",
			"params": { "name": "project.open", "arguments": [] },
		}))
		.unwrap();
	assert_eq!(response["error"]["code"], rpc::INVALID_PARAMS);
	assert!(
		response["error"]["message"]
			.as_str()
			.unwrap()
			.contains("array"),
		"message should name the offending type: {response}"
	);
}

#[test]
fn success_and_failure_share_the_tool_result_shape() {
	let mut server = server();
	init(&mut server);

	let ok = server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 10,
			"method": "tools/call",
			"params": { "name": "project.open", "arguments": { "name": "unit", "fps": 30 } },
		}))
		.unwrap();
	assert_eq!(ok["result"]["isError"], false);
	assert_eq!(ok["result"]["structuredContent"]["created"], true);
	assert_eq!(ok["result"]["content"][0]["type"], "text");

	// Schema-level rejection: no transaction is started.
	let bad = server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 11,
			"method": "tools/call",
			"params": {
				"name": "timeline.split_clip",
				"arguments": { "clip": "clip-1", "time": 1.5 },
			},
		}))
		.unwrap();
	assert_eq!(bad["result"]["isError"], true);
	let error = &bad["result"]["structuredContent"];
	assert_eq!(error["code"], "SCHEMA_VALIDATION_FAILED");
	assert_eq!(error["step"], "validate");
	assert_eq!(error["tool"], "timeline.split_clip");
	assert!(error.get("txn").is_none(), "schema failures open no txn");
}

#[test]
fn backend_failure_is_marked_aborted() {
	let mut server = server();
	init(&mut server);
	server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 12,
			"method": "tools/call",
			"params": { "name": "project.open", "arguments": { "name": "unit", "fps": 30 } },
		}))
		.unwrap();
	let response = server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 13,
			"method": "tools/call",
			"params": {
				"name": "timeline.split_clip",
				"arguments": { "clip": "clip-999", "time": { "num": 1, "den": 1 } },
			},
		}))
		.unwrap();
	let error = &response["result"]["structuredContent"];
	assert_eq!(response["result"]["isError"], true);
	assert_eq!(error["code"], "NOT_FOUND");
	assert_eq!(error["step"], "invoke");
	assert_eq!(error["txn"], "aborted");
}

#[test]
fn unknown_tool_keeps_the_structured_lookup_error() {
	let mut server = server();
	init(&mut server);
	let response = server
		.handle(&json!({
			"jsonrpc": "2.0",
			"id": 14,
			"method": "tools/call",
			"params": { "name": "timeline.teleport", "arguments": {} },
		}))
		.unwrap();
	assert_eq!(response["result"]["isError"], true);
	let error = &response["result"]["structuredContent"];
	assert_eq!(error["code"], "UNKNOWN_TOOL");
	assert_eq!(error["step"], "lookup");
}
