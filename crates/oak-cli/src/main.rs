// Oak Video Editor - Non-Linear Video Editor
// Copyright (C) 2026 Oak Team
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

//! oak-cli: headless command-line consumer of the oak editor modules.
//!
//! Rust rewrite of `cli/main.cpp` (which stays in the tree until cutover).
//! Same subcommands, same output format, same exit codes:
//!
//! ```text
//!   oak-cli info <project.ove> <start> <end> <out_dir>   (project info)
//!   oak-cli render <project.ove> <start_seconds> <end_seconds> <out_dir>
//!   oak-cli probe <mediafile>
//!   oak-cli transcode <input_media> <out> [width] [--format ppm|mp4]
//! ```
//!
//! Exit codes: 0 success, 1 general error, 2 rendering unavailable,
//! 64 usage error.
//!
//! Since M14 R2 this crate links the oak* module rlibs directly
//! (oaknode / oaktimeline / oakcodec / oakrender / oaktask / oakcommon) —
//! no liboakengine dylib, no C ABI, no host shims. [`engine`] is the
//! CLI's own assembly layer over the modules; the subcommands in
//! `src/cmd/` call it (and the modules) directly. [`fmt`], [`ppm`] and
//! [`wav`] are pure-Rust formatting/writing helpers (exact ports of the
//! C++ `printf`/writers).

mod cmd;
mod engine;
mod fmt;
mod ppm;
mod wav;

#[cfg(not(windows))]
#[used]
static FORCE_FFMPEG_LINK: fn() = oak_ffmpeg_link::force_link;

use std::process::exit;

use clap::{Parser, Subcommand};

/// The exact usage text of `cli/main.cpp`'s `print_usage()` (also the
/// `--help` output).
const USAGE: &str = "oak-cli - headless consumer of the oak editor modules (direct Rust ABI)\n\
\n\
Usage:\n\
  oak-cli info <project.ove>\n\
      Print project name, sequences and footage.\n\
\n\
  oak-cli render <project.ove> <start_seconds> <end_seconds> <out_dir>\n\
      Render the first sequence to PPM frames (P6, 8-bit RGB) and the\n\
      audio range to a PCM s16 WAV file in <out_dir>.\n\
\n\
  oak-cli probe <mediafile>\n\
      Probe a media file: decoder, duration, video and audio streams.\n\
\n\
  oak-cli transcode <input_media> <out> [width] [--format ppm|mp4]\n\
      Transcode a media file end to end: import it into a temporary\n\
      project, place it as clips, and render the whole duration.\n\
      Default output is a single H.264/AAC MP4 file (encoder default\n\
      bit rate); --format ppm renders PPM frames + a WAV instead.\n\
      [width] defaults to the source width; the height follows the\n\
      source aspect ratio. <out> is the MP4 file path, or the\n\
      output directory with --format ppm.\n\
\n\
  oak-cli --help\n\
      Show this text.\n\
\n\
Exit codes:\n\
  0   success\n\
  1   general error (bad project/media file, no sequence, I/O failure)\n\
  2   rendering unavailable or failed (e.g. no render backend)\n\
  64  usage error\n";

/// CLI surface. `--help`/`-h` are handled before clap so the C++ usage text
/// is reproduced exactly; clap still enforces the argument shapes.
#[derive(Parser, Debug)]
#[command(
	name = "oak-cli",
	disable_help_flag = true,
	disable_version_flag = true,
	subcommand_required = true
)]
struct Cli {
	#[command(subcommand)]
	command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
	/// Print project name, sequences and footage.
	Info {
		/// Path to the project file (.ove).
		project: String,
	},
	/// Render the first sequence to PPM frames (P6, 8-bit RGB) and the
	/// audio range to a PCM s16 WAV file in <out_dir>.
	Render {
		/// Path to the project file (.ove).
		project: String,
		/// Start of the rendered range, in seconds.
		start_seconds: String,
		/// End of the rendered range, in seconds (must be > start).
		end_seconds: String,
		/// Directory the PPM frames and audio.wav are written into.
		out_dir: String,
	},
	/// Probe a media file: decoder, duration, video and audio streams.
	Probe {
		/// Media file to probe.
		mediafile: String,
	},
	/// Transcode a media file end to end.
	Transcode {
		/// Source media file.
		input_media: String,
		/// Output MP4 path, or the output directory with --format ppm.
		out: String,
		/// Output width (defaults to the source width).
		width: Option<String>,
		/// Output format: "mp4" (default) or "ppm".
		#[arg(long = "format")]
		format: Option<String>,
	},
}

/// Runs the CLI: parses one subcommand, dispatches it, and exits with its code.
///
/// Arguments are read from `std::env::args()` with argv[0] skipped. As the first
/// argument, `--help`/`-h` prints [`USAGE`] and exits 0; any other first argument
/// outside `info`, `render`, `probe` and `transcode` prints an error plus the
/// usage text and exits with the usage code (64). Clap then validates the
/// remaining argument shapes — a parse error prints clap's message and the usage
/// text, also exiting 64. A valid command is dispatched to
/// `cmd::{info,render,probe,transcode}::run`, and the process exits with the code
/// that function returns (0 success, 1 general error, 2 rendering unavailable).
/// This function never returns; it must only be used as the process entry point.
fn main() {
	let args: Vec<String> = std::env::args().skip(1).collect();

	// argv[1] handling that mirrors the C++ main() exactly.
	if let Some(first) = args.first() {
		if first == "--help" || first == "-h" {
			print!("{USAGE}");
			exit(cmd::EXIT_OK);
		}
	}
	if let Some(first) = args.first() {
		if !matches!(first.as_str(), "info" | "render" | "probe" | "transcode") {
			eprintln!("error: unknown command \"{first}\"");
			eprint_usage();
			exit(cmd::EXIT_USAGE);
		}
	}

	let cli = match Cli::try_parse() {
		Ok(cli) => cli,
		Err(e) => {
			// clap's own arity/format message, then the C++ usage text.
			let _ = e.print();
			eprint_usage();
			exit(cmd::EXIT_USAGE);
		}
	};

	let code = match cli.command {
		Command::Info { project } => cmd::info::run(project),
		Command::Render {
			project,
			start_seconds,
			end_seconds,
			out_dir,
		} => cmd::render::run(project, &start_seconds, &end_seconds, &out_dir),
		Command::Probe { mediafile } => cmd::probe::run(mediafile),
		Command::Transcode {
			input_media,
			out,
			width,
			format,
		} => cmd::transcode::run(input_media, out, width, format),
	};
	exit(code);
}

fn eprint_usage() {
	eprint!("{USAGE}");
}
