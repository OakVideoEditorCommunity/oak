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

//! Subcommand implementations.
//!
//! Every subcommand is a real implementation over `quercus-engine-rs`:
//! the CLI links no other Oak crate.
//!
//!   - `probe`     → the SDK codec probe (`codec::decoder_probe`)
//!   - `info`      → the SDK project/session queries
//!   - `render`    → SDK frame/audio tasks written as PPM/WAV
//!   - `transcode` → a temporary SDK session + render/export tasks
//!
//! Exit codes are shared by every command (see [`EXIT_OK`],
//! [`EXIT_ERROR`], [`EXIT_USAGE`], [`EXIT_RENDER_UNAVAILABLE`]).

pub mod info;
pub mod probe;
pub mod render;
pub mod transcode;

/// 0 — success.
pub const EXIT_OK: i32 = 0;
/// 1 — general error (bad project/media file, no sequence, I/O failure).
pub const EXIT_ERROR: i32 = 1;
/// 2 — rendering unavailable or failed (e.g. no render backend).
pub const EXIT_RENDER_UNAVAILABLE: i32 = 2;
/// 64 — usage error.
pub const EXIT_USAGE: i32 = 64;
