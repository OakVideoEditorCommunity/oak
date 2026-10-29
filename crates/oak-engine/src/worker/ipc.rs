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

//! Render-worker IPC shim (M15 S1): the NDJSON protocol and the
//! shared-memory frame-slot transport now live in the oakrender crate
//! (`oak_render::ipc`) so both ends of the pipe link one copy — the
//! main-process dispatcher creates the segments and the worker attaches
//! to them. This module re-exports the whole surface, keeping the
//! worker-side `crate::worker::ipc::` paths unchanged. The facade (oakengine)
//! still keeps its own copy for the frozen `oakengine_ipc_*` C ABI.

pub use oak_render::ipc::*;
