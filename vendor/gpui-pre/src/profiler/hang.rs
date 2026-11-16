//! Post-hoc detection of foreground hangs from the journal's event stream.
//!
//! A hang is any single piece of foreground work — a task poll, an action
//! handler, an input dispatch, a window draw, or platform presentation — that
//! blocked the foreground thread for at least a threshold duration. An
//! interval whose total foreground spend reached the frame budget also
//! counts, even when no single event crossed the threshold: many small
//! pieces of work can drop a frame — or starve a headless app — as
//! thoroughly as one long stall. [`HangDetector`] drains the journal and
//! reports each completed activity interval containing hangs as a
//! [`HangIncident`].

use std::cmp;
use std::collections;
use std::mem;
use std::time::Duration;

use scheduler::Instant;
use serde::Serialize;

#[cfg(not(target_family = "wasm"))]
mod monitor;
#[cfg(not(target_family = "wasm"))]
pub(crate) use monitor::HangMonitor;
#[cfg(not(target_family = "wasm"))]
pub use monitor::{HangMonitorConfig, HangMonitorError, HangMonitorPoll, HangMonitorPollReason};

/// Version of the power/visibility-aware measurement rules.
pub const MEASUREMENT_VERSION: u32 = 3;

use super::SerializedLocation;
use super::journal::{
    ForegroundEvent, ForegroundJournal, ForegroundJournalCollector, ForegroundJournalEntry,
    FrameSnapshot, IntervalBoundary, IntervalSealer,
};

/// Detects foreground hangs by polling the journal.
///
/// Detection is post-hoc: a hang is reported once an explicit presentation,
/// completed frame skip, or foreground-idle boundary completes its interval.
/// Work that never yields back to the foreground is not observed until it does.
pub struct HangDetector {
    collector: ForegroundJournalCollector,
    sealer: IntervalSealer,
    threshold: Duration,
    frame_budget: Duration,
    first_present_at: Option<Instant>,
    activity: ActiveUse,
}

/// The user counts as actively using the app for this long after deliberate
/// input.
pub const ACTIVE_USE_WINDOW: Duration = Duration::from_secs(60);

/// Input that doesn't show the user is using the app: platforms deliver
/// pointer movement to background windows the pointer merely passes over.
const PASSIVE_INPUT_KINDS: [&str; 3] = ["mouse_move", "mouse_exited", "mouse_pressure"];

/// Accumulates the time the user was actively using the app: within
/// [`ACTIVE_USE_WINDOW`] of deliberate input, excluding stretches interrupted
/// by a system suspend.
#[derive(Default)]
struct ActiveUse {
    /// End times of recent deliberate input, oldest first, for
    /// [`Self::was_active_during`].
    recent_inputs: collections::VecDeque<Instant>,
    accounted_until: Option<Instant>,
    active_until: Option<Instant>,
    first_input_since_accounted: Option<Instant>,
    last_input_since_accounted: Option<Instant>,
    suspended: bool,
}

impl ActiveUse {
    fn observe(&mut self, entry: &ForegroundJournalEntry) {
        match entry {
            ForegroundJournalEntry::Event(ForegroundEvent::Input(input))
                if !PASSIVE_INPUT_KINDS.contains(&input.kind) =>
            {
                self.recent_inputs.push_back(input.end);
                self.first_input_since_accounted.get_or_insert(input.start);
                self.last_input_since_accounted = Some(
                    self.last_input_since_accounted
                        .map_or(input.end, |last| last.max(input.end)),
                );
            }
            ForegroundJournalEntry::Boundary(IntervalBoundary::PowerTransition { .. }) => {
                self.suspended = true;
            }
            _ => {}
        }
    }

    /// Whether the user was actively using the app at some point from
    /// `start` to `end`: deliberate input during that span or within
    /// [`ACTIVE_USE_WINDOW`] before it.
    fn was_active_during(&self, start: Instant, end: Instant) -> bool {
        let since = start.checked_sub(ACTIVE_USE_WINDOW).unwrap_or(start);
        self.recent_inputs
            .iter()
            .any(|input| *input >= since && *input <= end)
    }

    fn take(&mut self, now: Instant, max_elapsed: Duration) -> Duration {
        // Incidents are judged soon after they end; inputs older than this
        // can't be within the active window of anything still being sealed.
        let retained_since = now.checked_sub(ACTIVE_USE_WINDOW * 10);
        while self
            .recent_inputs
            .front()
            .is_some_and(|input| retained_since.is_some_and(|since| *input < since))
        {
            self.recent_inputs.pop_front();
        }
        let since = self.accounted_until.replace(now);
        let previous_until = self.active_until;
        let first_input = self.first_input_since_accounted.take();
        if let Some(last_input) = self.last_input_since_accounted.take() {
            let until = last_input + ACTIVE_USE_WINDOW;
            self.active_until = Some(previous_until.map_or(until, |previous| previous.max(until)));
        }
        let (Some(since), false) = (since, mem::take(&mut self.suspended)) else {
            return Duration::ZERO;
        };
        let continued_until = previous_until.map_or(since, |until| until.min(now).max(since));
        let mut active = continued_until.duration_since(since);
        if let (Some(first_input), Some(active_until)) = (first_input, self.active_until) {
            let start = first_input.max(continued_until);
            active += active_until.min(now).saturating_duration_since(start);
        }
        active.min(max_elapsed)
    }
}

/// One sealed interval that contained at least one hang.
#[derive(Debug, Clone)]
pub struct HangIncident {
    /// The interval the hangs occurred in, including all non-hang foreground
    /// work recorded alongside them.
    pub snapshot: FrameSnapshot,
    /// Which detection rule qualified the interval.
    pub trigger: HangTrigger,
    /// The events that caused the incident, longest first: those at least as
    /// long as the threshold or the frame budget for [`HangTrigger::Threshold`]
    /// and [`HangTrigger::BlockingWork`], and those overlapping the late frame's
    /// work for [`HangTrigger::LateFrame`].
    pub contributors: Vec<ForegroundEvent>,
    /// Whether the user was actively using the app during the incident (see
    /// [`HangDetector::take_active_time`]), so rates per active hour can count
    /// only incidents that happened during it.
    pub during_active_use: bool,
}

/// The detection rule that qualified an interval as a [`HangIncident`].
///
/// Recorded explicitly so consumers can separate the two classes without
/// re-deriving them from `stall_ms`, which stops working whenever the
/// detector's thresholds change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HangTrigger {
    /// A single event blocked the foreground for at least the hang threshold.
    Threshold,
    /// The interval presented a frame whose work exceeded its budget (see
    /// [`FrameTiming::is_late`](crate::profiler::FrameTiming::is_late)).
    LateFrame,
    /// The interval presented no frame whose work could be measured, but a
    /// single event blocked the foreground for at least the frame budget,
    /// long enough to delay a frame.
    BlockingWork,
}

impl HangTrigger {
    /// Every trigger.
    pub const ALL: [Self; 3] = [Self::Threshold, Self::LateFrame, Self::BlockingWork];

    /// The trigger's serialized name, e.g. `"late_frame"`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Threshold => "threshold",
            Self::LateFrame => "late_frame",
            Self::BlockingWork => "blocking_work",
        }
    }
}

impl HangDetector {
    /// Creates a detector reporting single events at or above `threshold`,
    /// late frames, and blocking work of at least `frame_budget` (see
    /// [`HangIncident::detect`]).
    /// Only events recorded from this point on are observed.
    pub fn new(journal: ForegroundJournal, threshold: Duration, frame_budget: Duration) -> Self {
        Self {
            collector: journal.collector(),
            sealer: IntervalSealer::new(Instant::now()),
            threshold,
            frame_budget,
            first_present_at: None,
            activity: ActiveUse::default(),
        }
    }

    /// The time since the previous call that the user was actively using
    /// the app, as of the journal entries polled so far: within
    /// [`ACTIVE_USE_WINDOW`] of deliberate input, such as key presses, clicks,
    /// and scrolling, excluding polls that observed a system
    /// suspend. Capped at `max_elapsed`, so time this thread spent suspended
    /// isn't counted.
    pub fn take_active_time(&mut self, now: Instant, max_elapsed: Duration) -> Duration {
        self.activity.take(now, max_elapsed)
    }

    /// When the first newly drawn frame observed by this detector finished
    /// platform submission. `None` until a presentation boundary is observed.
    pub fn first_present_at(&self) -> Option<Instant> {
        self.first_present_at
    }

    /// Drains newly recorded events and returns the incidents sealed since
    /// the previous poll.
    pub fn poll(&mut self) -> Vec<HangIncident> {
        let drained = self.collector.collect_unseen();
        for entry in &drained.entries {
            self.activity.observe(entry);
        }
        if self.first_present_at.is_none() {
            self.first_present_at = drained.entries.iter().find_map(|entry| match entry {
                ForegroundJournalEntry::Boundary(IntervalBoundary::Presented(presented)) => {
                    Some(presented.presentation.present_end)
                }
                _ => None,
            });
        }
        self.sealer
            .push_entries(drained.entries)
            .into_iter()
            .filter_map(|snapshot| {
                let mut incident =
                    HangIncident::detect(snapshot, self.threshold, self.frame_budget)?;
                let (start, end) = incident.active_window();
                incident.during_active_use = self.activity.was_active_during(start, end);
                Some(incident)
            })
            .collect()
    }
}

/// A [`HangIncident`] in a telemetry-friendly form: timestamps and durations
/// in fractional milliseconds since app startup (microsecond precision),
/// locations as plain data, contributor count capped by the converter.
#[derive(Debug, Clone, Serialize)]
pub struct SerializedHangIncident {
    /// Identifies the rules used to exclude interrupted measurements.
    pub measurement_version: u32,
    /// `"startup"` when the active window began before the first observed
    /// newly drawn frame finished platform submission (see
    /// [`HangDetector::first_present_at`]), otherwise `"steady"`.
    pub phase: &'static str,
    /// `"threshold"`, `"late_frame"`, or `"blocking_work"` (see
    /// [`HangTrigger`]).
    pub trigger: HangTrigger,
    /// See [`HangIncident::during_active_use`].
    pub during_active_use: bool,
    /// When the incident's active window started, in milliseconds since app
    /// startup: the sealing frame's first invalidation, or the earliest
    /// contributor's start when nothing was pending a repaint. Foreground
    /// idle time between the previous frame and the cause is excluded.
    pub start_ms: f64,
    /// Length of the active window in milliseconds: from the cause to the
    /// seal. Exceeds `stall_ms` when several stalls piled up on one frame.
    pub active_ms: f64,
    /// The longest single block of foreground work, in milliseconds: the
    /// best estimate of the freeze a user perceived. Below the hang
    /// threshold (possibly zero) when the frame budget alone triggered the
    /// incident.
    pub stall_ms: f64,
    /// For presentation-sealed incidents, how long the submitted frame had
    /// been dirty, in milliseconds.
    pub dirty_to_present_ms: Option<f64>,
    /// What closed the incident: `"present"`, `"frame_skipped"`, `"idle"`, or
    /// `"power_transition"`. This labels the boundary, not the hang's cause —
    /// the cause is the first contributor.
    pub sealed_by: &'static str,
    /// Fraction of the active window the foreground spent working,
    /// `0.0..=1.0`. Low values with a high `dirty_to_present_ms` indicate
    /// throttling or scheduling delay rather than application work.
    pub busy_fraction: f64,
    /// Total events recorded in the interval (before the contributor cap).
    pub event_count: usize,
    /// Count of task polls below the journal's floor.
    pub small_poll_count: u64,
    /// Total duration of task polls below the journal's floor, in milliseconds.
    pub small_poll_total_ms: f64,
    /// Events lost to caps or ring overwrites.
    pub dropped_events: u64,
    /// Whether one or more journal entries were unavailable in the interval.
    /// Threshold-qualified contributors remain valid, but cumulative budget
    /// conclusions and boundary attribution are not trusted across the gap.
    pub journal_discontinuous: bool,
    /// The incident's contributors (see [`HangIncident::contributors`]) in
    /// start order. The cap keeps the longest ones; `stall_ms` is always
    /// among them.
    pub contributors: Vec<SerializedHangContributor>,
    /// Contributors elided by the cap.
    pub contributors_elided: usize,
}

/// One hang contributor in serialized form.
///
/// Foreground work nests: an input dispatch can synchronously draw a window,
/// a draw can poll a task. `depth` expresses that containment — a depth-1
/// event's time is already inside some depth-0 event's duration, so summing
/// sibling durations across depths double-counts.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SerializedHangContributor {
    /// A foreground task poll.
    TaskPoll {
        /// Where the task was spawned.
        location: SerializedLocation,
        /// When the poll started, in milliseconds since app startup.
        start_ms: f64,
        /// How long the poll blocked the foreground, in milliseconds.
        duration_ms: f64,
        /// How many other events in the interval contain this one.
        depth: usize,
    },
    /// An action handler.
    Action {
        /// The action's name.
        name: &'static str,
        /// When the handler started, in milliseconds since app startup.
        start_ms: f64,
        /// How long the handler ran, in milliseconds.
        duration_ms: f64,
        /// How many other events in the interval contain this one.
        depth: usize,
    },
    /// A platform input dispatch.
    Input {
        /// The platform input variant dispatched, e.g. `"key_down"`.
        /// Named `input_kind` because `kind` is this enum's serde tag.
        input_kind: &'static str,
        /// When the dispatch started, in milliseconds since app startup.
        start_ms: f64,
        /// How long the dispatch ran, in milliseconds.
        duration_ms: f64,
        /// Whether handling the input invalidated a window.
        caused_invalidation: bool,
        /// How many other events in the interval contain this one.
        depth: usize,
    },
    /// A window draw.
    Draw {
        /// The window that was drawn.
        window_id: u64,
        /// When the draw started, in milliseconds since app startup.
        start_ms: f64,
        /// How long the draw took, in milliseconds.
        duration_ms: f64,
        /// Time from the frame's first invalidation to the end of its draw,
        /// in milliseconds.
        dirty_to_draw_ms: Option<f64>,
        /// Invalidations coalesced into the frame.
        invalidations: u64,
        /// Time spent rendering views and requesting layout, in milliseconds.
        request_layout_ms: f64,
        /// Time spent computing layout and prepainting, in milliseconds.
        prepaint_ms: f64,
        /// Time spent painting, in milliseconds.
        paint_ms: f64,
        /// How many other events in the interval contain this one.
        depth: usize,
    },
    /// One phase of drawing a view whose own work took at least
    /// [`crate::profiler::VIEW_TIMING_FLOOR`].
    View {
        /// The view's type name.
        name: &'static str,
        /// The draw phase: `"request_layout"`, `"prepaint"`, or `"paint"`.
        phase: &'static str,
        /// When the phase started, in milliseconds since app startup.
        start_ms: f64,
        /// How long the phase took, including nested views, in milliseconds.
        duration_ms: f64,
        /// How long the phase took excluding nested views, in milliseconds.
        self_ms: f64,
        /// How many other events in the interval contain this one.
        depth: usize,
    },
    /// Work spent submitting a frame to the platform.
    Present {
        /// The window whose frame was submitted.
        window_id: u64,
        /// When submission began, in milliseconds since app startup.
        start_ms: f64,
        /// How long platform submission took, in milliseconds.
        duration_ms: f64,
        /// How many other events in the interval contain this one.
        depth: usize,
    },
}

/// Milliseconds with microsecond precision: keeps `dbg!`/JSON output short
/// (`115.954` rather than `115.95400000000001` or `115954`).
fn as_millis(duration: Duration) -> f64 {
    duration.as_micros() as f64 / 1000.0
}

impl SerializedHangIncident {
    /// Converts an incident, keeping at most `max_contributors` contributors.
    /// `first_present_at` is the end of the first observed newly drawn frame's
    /// platform submission (typically [`HangDetector::first_present_at`]);
    /// incidents whose active window begins before it are tagged `"startup"`.
    pub fn convert(
        startup: Instant,
        incident: &HangIncident,
        max_contributors: usize,
        first_present_at: Option<Instant>,
    ) -> Self {
        let since_startup =
            |instant: Instant| as_millis(instant.saturating_duration_since(startup));
        let snapshot = &incident.snapshot;
        let (active_start, active_end) = incident.active_window();
        let active = active_end.duration_since(active_start);
        let busy_fraction = if active.is_zero() {
            1.0
        } else {
            snapshot
                .occupancy_within(active_start, active_end)
                .div_duration_f64(active)
                .min(1.0)
        };
        Self {
            phase: match first_present_at {
                Some(first_present_at) if active_start >= first_present_at => "steady",
                _ => "startup",
            },
            measurement_version: MEASUREMENT_VERSION,
            trigger: incident.trigger,
            during_active_use: incident.during_active_use,
            start_ms: since_startup(active_start),
            active_ms: as_millis(active),
            stall_ms: incident
                .contributors
                .first()
                .map(|event| as_millis(event.duration()))
                .unwrap_or(0.0),
            dirty_to_present_ms: match snapshot.boundary {
                IntervalBoundary::Presented(presented) => {
                    presented.dirty_to_present_duration().map(as_millis)
                }
                IntervalBoundary::FrameSkipped(_)
                | IntervalBoundary::Idle { .. }
                | IntervalBoundary::PowerTransition { .. } => None,
            },
            sealed_by: match snapshot.boundary {
                IntervalBoundary::Presented(_) => "present",
                IntervalBoundary::FrameSkipped(_) => "frame_skipped",
                IntervalBoundary::Idle { .. } => "idle",
                IntervalBoundary::PowerTransition { .. } => "power_transition",
            },
            busy_fraction: (busy_fraction * 1000.0).round() / 1000.0,
            event_count: snapshot.events.len(),
            small_poll_count: snapshot.small_poll_summary().count,
            small_poll_total_ms: as_millis(snapshot.small_poll_summary().total),
            dropped_events: snapshot.dropped_events,
            journal_discontinuous: snapshot.journal_discontinuous,
            contributors: {
                let mut kept: Vec<&ForegroundEvent> = incident
                    .contributors
                    .iter()
                    .take(max_contributors)
                    .collect();
                kept.sort_by_key(|event| event.start_time());
                kept.into_iter()
                    .map(|event| {
                        SerializedHangContributor::convert(
                            startup,
                            event,
                            nesting_depth(event, &snapshot.events),
                        )
                    })
                    .collect()
            },
            contributors_elided: incident.contributors.len().saturating_sub(max_contributors),
        }
    }
}

/// How many events in `events` strictly contain `event`'s span. Events with
/// identical spans don't count as containing each other, so `event`'s own
/// presence in `events` contributes nothing.
fn nesting_depth(event: &ForegroundEvent, events: &[ForegroundEvent]) -> usize {
    let (start, end) = (event.start_time(), event.end_time());
    events
        .iter()
        .filter(|other| {
            let (other_start, other_end) = (other.start_time(), other.end_time());
            other_start <= start && end <= other_end && (other_start < start || end < other_end)
        })
        .count()
}

impl SerializedHangContributor {
    fn convert(startup: Instant, event: &ForegroundEvent, depth: usize) -> Self {
        let since_startup =
            |instant: Instant| as_millis(instant.saturating_duration_since(startup));
        let duration_ms = as_millis(event.duration());
        match event {
            ForegroundEvent::TaskPoll(timing) => Self::TaskPoll {
                location: timing.location.into(),
                start_ms: since_startup(timing.start),
                duration_ms,
                depth,
            },
            ForegroundEvent::Action(timing) => Self::Action {
                name: timing.name,
                start_ms: since_startup(timing.start),
                duration_ms,
                depth,
            },
            ForegroundEvent::Input(timing) => Self::Input {
                input_kind: timing.kind,
                start_ms: since_startup(timing.start),
                duration_ms,
                caused_invalidation: timing.caused_invalidation,
                depth,
            },
            ForegroundEvent::Draw(timing) => Self::Draw {
                window_id: timing.window_id.as_u64(),
                start_ms: since_startup(timing.draw_start),
                duration_ms,
                dirty_to_draw_ms: timing.dirty_to_draw_duration().map(as_millis),
                invalidations: timing.invalidations,
                request_layout_ms: as_millis(timing.phases.request_layout),
                prepaint_ms: as_millis(timing.phases.prepaint),
                paint_ms: as_millis(timing.phases.paint),
                depth,
            },
            ForegroundEvent::View(timing) => Self::View {
                name: timing.name,
                phase: timing.phase.as_str(),
                start_ms: since_startup(timing.start),
                duration_ms,
                self_ms: as_millis(timing.self_duration),
                depth,
            },
            ForegroundEvent::Present(timing) => Self::Present {
                window_id: timing.window_id.as_u64(),
                start_ms: since_startup(timing.present_start),
                duration_ms,
                depth,
            },
            ForegroundEvent::SmallPolls(flush) => {
                // The sealer folds these out of snapshot events; a contributor
                // can therefore never be one, but serialize defensively as an
                // unnamed poll spanning the flush rather than panicking.
                Self::TaskPoll {
                    location: SerializedLocation {
                        file: "<small poll summary>".into(),
                        line: 0,
                        column: 0,
                    },
                    start_ms: since_startup(flush.since),
                    duration_ms,
                    depth,
                }
            }
        }
    }
}

impl HangIncident {
    /// The incident's reporting window: from its earliest cause — the
    /// sealing frame's first invalidation, or the earliest contributor's
    /// start when no repaint was pending — to the seal. This trims
    /// foreground-idle time between the previous frame and the cause, and
    /// may begin before the underlying snapshot when a contributor was
    /// already running at the previous seal.
    pub fn active_window(&self) -> (Instant, Instant) {
        let snapshot = &self.snapshot;
        let dirty_at = snapshot.boundary.dirty_at();
        let earliest_contributor = self
            .contributors
            .iter()
            .map(|event| event.start_time())
            .min();
        let start = [dirty_at, earliest_contributor]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(snapshot.interval_start);
        (start, snapshot.interval_end())
    }

    /// Returns an incident when the snapshot contains an event that blocked
    /// the foreground for `threshold` or longer; otherwise when the interval
    /// presented a late frame, unless the journal lost events; otherwise, when
    /// no presented frame's work could be measured, when an event blocked the
    /// foreground for `frame_budget` or longer. See [`HangTrigger`].
    pub fn detect(
        snapshot: FrameSnapshot,
        threshold: Duration,
        frame_budget: Duration,
    ) -> Option<Self> {
        let events_at_least = |minimum: Duration| -> Vec<ForegroundEvent> {
            snapshot
                .events
                .iter()
                .filter(|event| event.duration() >= minimum)
                .copied()
                .collect()
        };
        let mut contributors = events_at_least(threshold);
        let measured_frame = match &snapshot.boundary {
            IntervalBoundary::Presented(presented) => presented
                .frame
                .work_start()
                .map(|work_start| (presented, work_start)),
            _ => None,
        };
        let trigger = if !contributors.is_empty() {
            HangTrigger::Threshold
        } else if let Some((presented, work_start)) = measured_frame {
            let submitted_at = presented.presentation.present_start;
            if snapshot.journal_discontinuous
                || !presented.frame.is_late(submitted_at).unwrap_or(false)
            {
                return None;
            }
            contributors = snapshot
                .events
                .iter()
                .filter(|event| event.end_time() > work_start && event.start_time() < submitted_at)
                .copied()
                .collect();
            HangTrigger::LateFrame
        } else {
            contributors = events_at_least(frame_budget);
            if contributors.is_empty() {
                return None;
            }
            HangTrigger::BlockingWork
        };
        contributors.sort_by_key(|event| cmp::Reverse(event.duration()));
        Some(Self {
            snapshot,
            trigger,
            contributors,
            during_active_use: false,
        })
    }
}

#[cfg(test)]
mod tests;
