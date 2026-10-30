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

//! oak-worker: a thin launcher for the engine's render-worker runtime.
//!
//! The NDJSON control loop, shared-memory frame-slot transport, graph
//! loading and per-process rendering live in the external `quercus-engine` crate; this binary only
//! depends on `quercus-engine-rs` and calls
//! [`quercus_engine_rs::worker::main`].

fn main() {
	std::process::exit(quercus_engine_rs::worker::main());
}
