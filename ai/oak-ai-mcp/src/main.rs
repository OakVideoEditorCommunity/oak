//! `oak-ai-mcp` — MCP stdio server for the Oak AI editing tool face.
//!
//! Usage: run with `--help`. The process speaks MCP over stdio (JSON-RPC 2.0, one document per
//! NDJSON line, on stdin/stdout; human-readable diagnostics go to stderr
//! only, so stdout stays a clean frame stream.

use clap::Parser as _;
use std::process::ExitCode;

use oak_ai_mcp::backend_select::{self, BackendKind};
use oak_ai_mcp::rmcp_server::OakMcp;
use oak_ai_mcp::server::Server;
use rmcp::ServiceExt as _;

/// Command line. `--backend` defaults to mock; `sdk` needs the feature.
#[derive(Debug, clap::Parser)]
#[command(
	name = "oak-ai-mcp",
	version,
	about = "MCP (stdio) server for the Oak AI editing tool face",
	long_about = "Speaks JSON-RPC 2.0, one object per line (NDJSON) on stdin/stdout.\nMethods: initialize, notifications/initialized, tools/list, tools/call, ping."
)]
struct Cli {
	/// Execution backend: mock (in-memory, no engine) or sdk (needs --features sdk).
	#[arg(long, value_name = "mock|sdk", default_value = "mock", value_parser = BackendKind::parse)]
	backend: BackendKind,
}

fn main() -> ExitCode {
	// clap owns --help/--version and the exit-2 usage error path.
	let kind = Cli::parse().backend;
	let backend = match backend_select::build(kind) {
		Ok(backend) => backend,
		Err(message) => {
			eprintln!("oak-ai-mcp: {message}");
			return ExitCode::from(2);
		}
	};

	eprintln!(
		"oak-ai-mcp {} (MCP {}, backend {}, {} tools)",
		env!("CARGO_PKG_VERSION"),
		oak_ai_mcp::server::BASELINE_PROTOCOL_VERSION,
		kind.label(),
		oak_ai_tools::dispatch::registry().len(),
	);

	let server = OakMcp::new(Server::new(backend, kind.label()));
	// The SDK drives the stdio transport; a current-thread runtime is enough
	// for a single-connection server.
	let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
		Ok(runtime) => runtime,
		Err(err) => {
			eprintln!("oak-ai-mcp: cannot start the async runtime: {err}");
			return ExitCode::from(1);
		}
	};
	// `local` feature: the engine backend is not `Send`, so the handler runs on
	// a LocalSet rather than a Send-requiring task.
	let local = tokio::task::LocalSet::new();
	runtime.block_on(local.run_until(async move {
		match server.serve(rmcp::transport::stdio()).await {
			Ok(service) => {
				let _ = service.waiting().await;
				ExitCode::SUCCESS
			}
			Err(err) => {
				eprintln!("oak-ai-mcp: transport failed: {err}");
				ExitCode::from(1)
			}
		}
	}))
}
