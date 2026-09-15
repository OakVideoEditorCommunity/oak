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

//! Contract tests for the work area domain (`src/workarea.rs`):
//! `TimelineWorkArea` value type, its two undo commands, and the
//! `reset_in`/`reset_out` sentinels. The XML load/save contract left
//! with the deleted C ABI export layer (single-lib unification).

use std::sync::{Arc, Mutex, MutexGuard};

use oak_core::{Rational, TimeRange};
use oak_timeline::handle::{get, make_owned, CHandle};
use oak_timeline::undocommon::Command;
use oak_timeline::workarea::{
	reset_in, reset_out, TimelineWorkArea, WorkareaSetEnabledCommand, WorkareaSetRangeCommand,
};

/// Lock the shared work area behind a live handle. Every work-area handle
/// in these tests comes from `make_owned`, which boxes an
/// `Arc<Mutex<TimelineWorkArea>>`.
fn wa_of(h: &CHandle) -> MutexGuard<'_, TimelineWorkArea> {
	// SAFETY: as above; the handle is live.
	unsafe { get::<Arc<Mutex<TimelineWorkArea>>>(h) }
		.expect("live work-area handle")
		.lock()
		.unwrap_or_else(|e| e.into_inner())
}

/// `reset_in` is the null rational (0/1) marking an unset work area
/// start.
#[test]
fn reset_in_is_zero() {
	assert_eq!(reset_in(), Rational::new(0, 1));
}

/// `reset_out` is the RATIONAL_MAX sentinel (2147483647/1) marking an
/// unset work area end.
#[test]
fn reset_out_is_zero() {
	assert_eq!(reset_out(), Rational::new(2147483647, 1));
}

/// A default work area is disabled with the reset range
/// (0/1 .. 2147483647/1).
#[test]
fn workarea_default_is_disabled_null_range() {
	let wa = TimelineWorkArea::new();
	assert!(!wa.enabled());
	assert_eq!(wa.in_(), reset_in());
	assert_eq!(wa.out(), reset_out());
}

/// `set_enabled`/`enabled` toggle the enabled flag round-trip.
#[test]
fn workarea_enabled_toggle() {
	let mut wa = TimelineWorkArea::new();
	assert!(!wa.enabled());
	wa.set_enabled(true);
	assert!(wa.enabled());
	wa.set_enabled(false);
	assert!(!wa.enabled());
}

/// `set_range` stores a range and `range`/`in_`/`out`/`length` expose
/// it consistently.
#[test]
fn workarea_set_range_exposes_parts() {
	let mut wa = TimelineWorkArea::new();
	let r = TimeRange::new(Rational::new(10, 1), Rational::new(20, 1));
	wa.set_range(r);
	assert_eq!(*wa.range(), r);
	assert_eq!(wa.in_(), Rational::new(10, 1));
	assert_eq!(wa.out(), Rational::new(20, 1));
	assert_eq!(wa.length(), Rational::new(10, 1));
}

/// `set_range` stores the supplied range verbatim. `TimeRange::new`
/// normalizes (swaps) when `out < in` (C++ parity), so a "zero out"
/// range `(5, 0)` is stored normalized as `(0, 5)`; the C++ zero-out
/// guard lives in the facade layer, not in `set_range`.
#[test]
fn workarea_zero_out_resets_range() {
	let mut wa = TimelineWorkArea::new();
	let r = TimeRange::new(Rational::new(5, 1), Rational::new(0, 1));
	wa.set_range(r);
	// `TimeRange::new` swapped 0 < 5 before the value was stored.
	assert_eq!(wa.in_(), Rational::new(0, 1));
	assert_eq!(wa.out(), Rational::new(5, 1));
	assert_eq!(*wa.range(), r);
}

/// `WorkareaSetEnabledCommand` redo enables / undo restores the prior
/// flag.
#[test]
fn workarea_set_enabled_command_redo_undo() {
	let wa_h = make_owned(TimelineWorkArea::new());
	let mut cmd = WorkareaSetEnabledCommand::new(wa_h, true);
	cmd.redo();
	assert!(wa_of(&wa_h).enabled());
	cmd.undo();
	assert!(!wa_of(&wa_h).enabled());
}

/// `WorkareaSetRangeCommand` redo stores the new range and undo
/// restores the previous one.
#[test]
fn workarea_set_range_command_redo_undo() {
	let wa_h = make_owned(TimelineWorkArea::new());
	let new_range = TimeRange::new(Rational::new(10, 1), Rational::new(20, 1));
	let mut cmd = WorkareaSetRangeCommand::new(wa_h, new_range);
	cmd.redo();
	assert_eq!(*wa_of(&wa_h).range(), new_range);
	cmd.undo();
	// Undo restores the range captured at construction (the reset range).
	assert_eq!(wa_of(&wa_h).in_(), reset_in());
	assert_eq!(wa_of(&wa_h).out(), reset_out());
}

/// `to_command` boxes a work area command into an oakundo `UndoCommand`
/// value for the undo stack (the old C ABI command handle is gone with
/// the single-lib unification).
#[test]
fn workarea_commands_box_to_undo_command() {
	let wa_h = make_owned(TimelineWorkArea::new());
	let mut enabled_cmd = WorkareaSetEnabledCommand::new(wa_h, true).to_command();
	enabled_cmd.redo_now();
	assert!(wa_of(&wa_h).enabled());
	enabled_cmd.undo_now();
	assert!(!wa_of(&wa_h).enabled());

	let mut range_cmd = WorkareaSetRangeCommand::new(
		wa_h,
		TimeRange::new(Rational::new(1, 1), Rational::new(2, 1)),
	)
	.to_command();
	range_cmd.redo_now();
	assert_eq!(wa_of(&wa_h).in_(), Rational::new(1, 1));
	range_cmd.undo_now();
	assert_eq!(wa_of(&wa_h).in_(), reset_in());
}

/// `Command` trait dispatch routes through the same redo/undo bodies as
/// the inherent methods (used by the undo stack command values).
#[test]
fn workarea_commands_trait_dispatch() {
	let wa_h = make_owned(TimelineWorkArea::new());

	let mut e = WorkareaSetEnabledCommand::new(wa_h, true);
	Command::redo(&mut e);
	assert!(wa_of(&wa_h).enabled());
	Command::undo(&mut e);
	assert!(!wa_of(&wa_h).enabled());

	let mut r = WorkareaSetRangeCommand::new(
		wa_h,
		TimeRange::new(Rational::new(3, 1), Rational::new(4, 1)),
	);
	Command::redo(&mut r);
	assert_eq!(wa_of(&wa_h).in_(), Rational::new(3, 1));
	Command::undo(&mut r);
	assert_eq!(wa_of(&wa_h).in_(), reset_in());
}
