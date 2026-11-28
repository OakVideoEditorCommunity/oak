//! Long-task progress events (capability group 3).
//!
//! Two producers, one shape: the agent loop reports *rounds* (`n/m` of
//! the LLM turns) and the plan runner reports *steps* (`n/m` of the
//! explicit plan). Events go two places — always into
//! [`crate::ai::agent::AgentReport::progress`] (so a report is complete
//! on its own) and, when a sink is wired with
//! [`crate::ai::agent::Agent::with_progress_sink`], to the subscriber
//! (the chat panel's spinner, a CLI ticker) as they happen.

use std::fmt;

/// One progress event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressEvent {
	/// 1-based current step (round or plan step).
	pub step: usize,
	/// Total number of steps in this phase.
	pub total: usize,
	/// What is running: `"round"` for an LLM turn, a plan step's title
	/// for plan execution, `"rollback"` while undoing.
	pub phase: String,
	/// Optional detail (the tool being called, the reason).
	pub detail: Option<String>,
}

impl ProgressEvent {
	/// Convenience constructor with no detail.
	pub fn new(step: usize, total: usize, phase: impl Into<String>) -> Self {
		Self {
			step,
			total,
			phase: phase.into(),
			detail: None,
		}
	}

	/// Attach a detail line.
	pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
		self.detail = Some(detail.into());
		self
	}

	/// Percentage complete, rounded; 100 when `total == 0` never happens
	/// (callers pass `total >= 1`), so this is total-safe.
	pub fn percent(&self) -> u8 {
		if self.total == 0 {
			return 0;
		}
		((self.step.min(self.total) * 100) / self.total).min(100) as u8
	}
}

impl fmt::Display for ProgressEvent {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			"[{}/{} {}%] {}",
			self.step,
			self.total,
			self.percent(),
			self.phase
		)?;
		if let Some(detail) = &self.detail {
			write!(f, " — {detail}")?;
		}
		Ok(())
	}
}

/// What subscribes to progress. The panel implements it directly; a
/// closure can be wrapped with [`FnSink`].
///
/// `Send` is a supertrait because the agent is spawned on a worker
/// thread by the chat panel — a sink that cannot cross threads would
/// make the whole `Agent` unspawnable.
pub trait ProgressSink: Send {
	/// Called as each step starts (before it executes, so a UI can show
	/// "step 3 of 7" while the work is still running).
	fn on_progress(&mut self, event: &ProgressEvent);
}

/// A closure as a sink.
pub struct FnSink<F>(pub F);

impl<F> ProgressSink for FnSink<F>
where
	F: FnMut(&ProgressEvent) + Send,
{
	fn on_progress(&mut self, event: &ProgressEvent) {
		(self.0)(event)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::{Arc, Mutex};

	/// ① Percentages are computed from step/total.
	#[test]
	fn percent_follows_the_step() {
		assert_eq!(ProgressEvent::new(1, 4, "round").percent(), 25);
		assert_eq!(ProgressEvent::new(4, 4, "round").percent(), 100);
		assert_eq!(ProgressEvent::new(0, 0, "round").percent(), 0);
	}

	/// ② Display is the line a CLI ticker would print.
	#[test]
	fn display_is_human_readable() {
		let event = ProgressEvent::new(3, 7, "place clips").with_detail("timeline.place_clip");
		assert_eq!(event.to_string(), "[3/7 42%] place clips — timeline.place_clip");
	}

	/// ③ A closure sink receives the event (the subscription seam).
	#[test]
	fn a_closure_sink_receives_events() {
		let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
		let sink_seen = Arc::clone(&seen);
		let mut sink = FnSink(move |event: &ProgressEvent| {
			sink_seen.lock().expect("poisoned").push(event.to_string());
		});
		sink.on_progress(&ProgressEvent::new(1, 2, "round"));
		sink.on_progress(&ProgressEvent::new(2, 2, "round"));
		assert_eq!(
			*seen.lock().expect("poisoned"),
			vec!["[1/2 50%] round".to_string(), "[2/2 100%] round".to_string()]
		);
	}
}
