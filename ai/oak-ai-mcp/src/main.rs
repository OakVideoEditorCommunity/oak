//! `oak-ai-mcp` — MCP stdio server for the Oak AI editing tool face.
//!
//! Usage: see [`USAGE`]. The process speaks JSON-RPC 2.0, one document per
//! NDJSON line, on stdin/stdout; human-readable diagnostics go to stderr
//! only, so stdout stays a clean frame stream.

use std::io::BufWriter;
use std::process::ExitCode;

use oak_ai_mcp::backend_select::{self, BackendKind};
use oak_ai_mcp::rpc::{self, Inbound};
use oak_ai_mcp::server::Server;

const USAGE: &str = "\
oak-ai-mcp — MCP (stdio) server for the Oak AI editing tool face

USAGE:
    oak-ai-mcp [--backend mock|sdk]

OPTIONS:
    --backend <mock|sdk>   Execution backend. Default: mock (in-memory, no engine).
                           'sdk' requires a build with --features sdk; without it the
                           server refuses to start rather than degrading to mock.
    -h, --help             Print this help.
    -V, --version          Print the version.

TRANSPORT:
    JSON-RPC 2.0, one object per line (NDJSON) on stdin/stdout, UTF-8.
    Methods: initialize, notifications/initialized, tools/list, tools/call, ping.
    Exit code 0 when the client closes stdin, 2 on a usage/startup error.

EXIT CODES:
    0  normal shutdown (stdin closed)
    2  bad arguments, unknown backend, or a backend that is not available
";

fn main() -> ExitCode {
	let backend = match parse_args() {
		Ok(Some(kind)) => kind,
		Ok(None) => return ExitCode::SUCCESS,
		Err(message) => {
			eprintln!("oak-ai-mcp: {message}");
			eprint!("{USAGE}");
			return ExitCode::from(2);
		}
	};

	let kind = backend;
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

	run(Server::new(backend, kind.label()))
}

/// `Ok(None)` means "handled, nothing to run" (`--help` / `--version`).
fn parse_args() -> Result<Option<BackendKind>, String> {
	let mut backend: Option<BackendKind> = None;
	let mut args = std::env::args().skip(1);
	while let Some(arg) = args.next() {
		match arg.as_str() {
			"-h" | "--help" => {
				print!("{USAGE}");
				return Ok(None);
			}
			"-V" | "--version" => {
				println!("oak-ai-mcp {}", env!("CARGO_PKG_VERSION"));
				return Ok(None);
			}
			"--backend" => {
				let value = args
					.next()
					.ok_or("--backend requires a value (mock|sdk)".to_string())?;
				backend = Some(BackendKind::parse(&value)?);
			}
			other => {
				if let Some(value) = other.strip_prefix("--backend=") {
					backend = Some(BackendKind::parse(value)?);
				} else {
					return Err(format!("unknown argument '{other}'"));
				}
			}
		}
	}
	Ok(Some(backend.unwrap_or(BackendKind::Mock)))
}

fn run(mut server: Server) -> ExitCode {
	let stdin = std::io::stdin();
	let mut reader = std::io::BufReader::new(stdin.lock());
	let stdout = std::io::stdout();
	let mut writer = BufWriter::new(stdout.lock());

	loop {
		let inbound = match rpc::read_inbound(&mut reader) {
			Ok(inbound) => inbound,
			Err(err) => {
				// The stream is unreadable (I/O failure or non-UTF-8 input);
				// continuing would desynchronise the frame stream.
				eprintln!("oak-ai-mcp: stdin read failed: {err}");
				return ExitCode::from(1);
			}
		};
		if inbound == Inbound::Eof {
			return ExitCode::SUCCESS;
		}
		let Some(response) = server.handle_inbound(inbound) else {
			continue;
		};
		if let Err(err) = rpc::write_frame(&mut writer, &response) {
			if err.kind() == std::io::ErrorKind::BrokenPipe {
				// The client went away mid-session; that is a normal end.
				return ExitCode::SUCCESS;
			}
			eprintln!("oak-ai-mcp: stdout write failed: {err}");
			return ExitCode::from(1);
		}
	}
}
