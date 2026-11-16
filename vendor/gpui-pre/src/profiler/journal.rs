//! A journal of foreground work and the semantic boundaries between activity intervals.
//!
//! The foreground thread records task polls, action handlers, input dispatches,
//! draws, and presentations as [`ForegroundEvent`]s in a bounded journal ring.
//! Task polls shorter than [`TASK_POLL_FLOOR`] are folded into summaries, which
//! bound the stream by the number of slow polls while preserving their exact
//! count and total duration.
//!
//! Presentation, completed frame skips, and foreground idle are explicit
//! [`IntervalBoundary`] entries in the same stream. Independent [`ForegroundJournalCollector`]s
//! feed entries to [`IntervalSealer`], a pure state machine that groups all work
//! preceding each boundary into a [`FrameSnapshot`]. The sealer does not infer
//! boundaries from elapsed time or from incidental event kinds such as draws.

use std::cell::{RefCell, UnsafeCell};
use std::collections::{HashMap, VecDeque};
use std::mem::MaybeUninit;
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};
use std::time::Duration;

use gpui_util::ResultExt;
use scheduler::Instant;

use super::{ActionTiming, FrameTiming, PresentTiming, TaskTiming, ViewTiming};
use crate::{App, WindowId, WindowVisibility};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PowerState {
    Awake,
    Suspended,
}

/// Task polls shorter than this are folded into a [`PollSummary`] instead of
/// being recorded individually. This keeps the stream bounded by the number
/// of *slow* polls while preserving their exact count and total duration.
pub const TASK_POLL_FLOOR: Duration = Duration::from_micros(100);

/// A dirty frame that has not been presented by this deadline stops blocking
/// foreground-idle boundaries, so a window that never presents (hidden or
/// occluded windows receive no frame callbacks) cannot suppress boundaries
/// indefinitely. Expiry only unblocks: the open interval still seals at a
/// presentation, completed frame skip, or idle boundary, keeping a starving
/// hang and the callback it starved in one interval.
pub const FRAME_DEADLINE: Duration = Duration::from_secs(1);

/// Growth in [`system_suspended_time`] below this is attributed to reading
/// its two clocks separately (on Windows both advance only on the system
/// tick, up to 15.6 ms), not to a suspend. A real suspend, including freezing
/// and thawing processes around it, takes far longer.
const MIN_DETECTED_SUSPEND: Duration = Duration::from_millis(50);

// Backstop against pathological event storms within a single interval. At the
// 100us floor, a fully hung second can produce at most ~10k recordable polls,
// so this bound is only reachable when something is already deeply wrong.
const MAX_INTERVAL_EVENTS: usize = 16 * 1024;

// Allow 4MiB for the fixed ring allocation, including each slot's atomic
// metadata. The poll floor and frame cadence bound the event rate to roughly
// 10k per second in the worst case, so this holds several seconds of
// worst-case traffic between consumer drains.
const MAX_JOURNAL_ENTRIES: usize = (4 * 1024 * 1024) / core::mem::size_of::<JournalSlot>();

// Absorbs brief collisions with a collector reading the exact slot being
// wrapped. The foreground never waits for a reader; queued entries are retried
// in order on the next publication.
const MAX_PENDING_JOURNAL_ENTRIES: usize = 64;

/// One entry in the foreground stream.
///
/// Events are recorded in completion order (ordered by their end time). An
/// event that was in progress across a frame boundary (e.g. the task poll
/// enclosing a draw) is recorded when it *ends*, with timestamps that may
/// precede events recorded before it.
#[derive(Debug, Copy, Clone)]
pub enum ForegroundEvent {
    /// A foreground task poll at least [`TASK_POLL_FLOOR`] long.
    TaskPoll(TaskTiming),
    /// A completed action handler.
    Action(ActionTiming),
    /// A dispatched platform input event.
    Input(InputTiming),
    /// A completed window draw.
    Draw(FrameTiming),
    /// One phase of drawing a view, recorded during the draw that contains it.
    View(ViewTiming),
    /// Work spent submitting a frame to the platform.
    Present(PresentTiming),
    /// Aggregate of task polls below [`TASK_POLL_FLOOR`], flushed before the
    /// next individually retained event or interval boundary.
    SmallPolls(SmallPollFlush),
}

impl ForegroundEvent {
    /// When the work described by this event began. For draws this is the
    /// start of the draw itself, not the frame's first invalidation.
    pub fn start_time(&self) -> Instant {
        match self {
            Self::TaskPoll(timing) => timing.start,
            Self::Action(timing) => timing.start,
            Self::Input(timing) => timing.start,
            Self::Draw(timing) => timing.draw_start,
            Self::View(timing) => timing.start,
            Self::Present(timing) => timing.present_start,
            Self::SmallPolls(flush) => flush.since,
        }
    }

    /// When the work described by this event ended.
    pub fn end_time(&self) -> Instant {
        match self {
            Self::TaskPoll(timing) => timing.end.0,
            Self::Action(timing) => timing.end,
            Self::Input(timing) => timing.end,
            Self::Draw(timing) => timing.draw_end,
            Self::View(timing) => timing.end,
            Self::Present(timing) => timing.present_end,
            Self::SmallPolls(flush) => flush.until,
        }
    }

    /// How long the work described by this event took. For
    /// [`Self::SmallPolls`] this is the span the summary covers, not time
    /// spent polling; use the summary's `total` for occupancy.
    pub fn duration(&self) -> Duration {
        self.end_time().duration_since(self.start_time())
    }
}

/// Timing of one platform input dispatch on a window.
#[derive(Debug, Copy, Clone)]
pub struct InputTiming {
    /// The platform input variant dispatched (see
    /// [`crate::PlatformInput::kind_name`]).
    pub kind: &'static str,
    /// When the input dispatch started.
    pub start: Instant,
    /// When the input dispatch finished.
    pub end: Instant,
    /// Whether handling the input invalidated a window.
    pub caused_invalidation: bool,
}

/// A frame callback entering the foreground, without implying foreground work.
#[derive(Debug, Copy, Clone)]
pub struct PlatformSignal {
    /// The window receiving the callback.
    pub window_id: WindowId,
    /// When the platform requested the callback, if its clock is supported.
    pub signal_at: Option<Instant>,
    /// When foreground handling began.
    pub handled_at: Instant,
    /// Whether the callback came from the platform or local scheduling.
    pub source: crate::FrameRequestSource,
}

/// Why a completed frame request did not draw a new frame.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FrameSkipReason {
    /// The callback completed without needing a new render.
    NoRenderNeeded,
    /// An inactive window deferred drawing.
    InactiveFrameRateLimit,
    /// Thermal throttling deferred drawing.
    ThermalFrameRateLimit,
}

/// A completed frame request that returned without drawing a new frame.
#[derive(Debug, Copy, Clone)]
pub struct FrameSkipped {
    /// The window whose callback returned.
    pub window_id: WindowId,
    /// When the foreground completed this callback.
    pub at: Instant,
    /// Why no new frame was drawn.
    pub reason: FrameSkipReason,
}

/// Exact count and total duration of task polls below [`TASK_POLL_FLOOR`].
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct PollSummary {
    /// Number of polls below the floor.
    pub count: u64,
    /// Total duration of polls below the floor.
    pub total: Duration,
}

impl PollSummary {
    fn add(&mut self, other: Self) {
        self.count += other.count;
        self.total += other.total;
    }
}

/// A flushed [`PollSummary`] and the tightest span containing its folded polls.
#[derive(Debug, Copy, Clone)]
pub struct SmallPollFlush {
    /// The folded polls.
    pub summary: PollSummary,
    /// When the first folded poll began.
    pub since: Instant,
    /// When the last folded poll ended.
    pub until: Instant,
}

/// A newly drawn frame and the platform submission that completed its interval.
#[derive(Debug, Copy, Clone)]
pub struct PresentedFrame {
    /// The draw whose rendered scene was submitted.
    pub frame: FrameTiming,
    /// Work spent submitting that scene to the platform.
    pub presentation: PresentTiming,
}

impl PresentedFrame {
    /// Time from the frame's first invalidation through platform submission.
    pub fn dirty_to_present_duration(&self) -> Option<Duration> {
        self.frame
            .dirty_at
            .map(|dirty_at| self.presentation.present_end.duration_since(dirty_at))
    }
}

/// The semantic event that completed a foreground activity interval.
#[derive(Debug, Copy, Clone)]
pub enum IntervalBoundary {
    /// A newly drawn frame was submitted to the platform.
    Presented(PresentedFrame),
    /// A callback completed without drawing, excluding reentrant callbacks.
    FrameSkipped(FrameSkipped),
    /// The foreground returned to an idle platform loop with no unexpired
    /// frame pending.
    Idle {
        /// When the foreground went idle.
        ended_at: Instant,
    },
    /// A system suspend interrupted the measurement interval, observed either
    /// through a power notification or through the suspend clock.
    PowerTransition {
        /// When the foreground noticed the suspend, through a notification or
        /// the suspend clock.
        ended_at: Instant,
    },
}

impl IntervalBoundary {
    /// When the interval ended.
    pub fn end_time(&self) -> Instant {
        match self {
            Self::Presented(presented) => presented.presentation.present_end,
            Self::FrameSkipped(skipped) => skipped.at,
            Self::Idle { ended_at } | Self::PowerTransition { ended_at } => *ended_at,
        }
    }

    /// The first invalidation of the frame satisfied by this boundary, if any.
    pub fn dirty_at(&self) -> Option<Instant> {
        match self {
            Self::Presented(presented) => presented.frame.dirty_at,
            Self::FrameSkipped(_) | Self::Idle { .. } | Self::PowerTransition { .. } => None,
        }
    }
}

/// A control-plane change to one window's pending-frame state.
#[derive(Debug, Copy, Clone)]
pub enum FrameStateChange {
    /// The window has a frame waiting to be presented.
    Pending {
        /// The window waiting for presentation.
        window_id: WindowId,
        /// When this frame generation first became dirty.
        dirty_at: Instant,
    },
    /// The window closed, so any pending frame no longer blocks idle
    /// boundaries.
    Closed {
        /// The window that closed.
        window_id: WindowId,
        /// When the window closed.
        at: Instant,
    },
}

/// One item retained in the foreground journal.
#[derive(Debug, Copy, Clone)]
pub enum ForegroundJournalEntry {
    /// A completed piece of foreground work.
    Event(ForegroundEvent),
    /// A semantic interval boundary.
    Boundary(IntervalBoundary),
    /// A change to pending-frame state. This is metadata, not foreground work.
    FrameState(FrameStateChange),
    /// A callback entering the foreground; not work or an interval boundary.
    PlatformSignal(PlatformSignal),
    /// A nested frame request deferred while an enclosing draw was still running.
    ReentrantFrameSkipped {
        /// The window whose nested frame request was deferred.
        window_id: WindowId,
        /// When the nested frame request was deferred.
        at: Instant,
    },
    /// One or more logical entries were unavailable at this point in the
    /// stream. Consumers must not infer interval boundaries across this gap.
    Discontinuity {
        /// Number of unavailable logical entries.
        lost: u64,
    },
}

/// An immutable view of one sealed foreground interval, produced by
/// [`IntervalSealer`].
#[derive(Debug, Clone)]
pub struct FrameSnapshot {
    /// When the interval started (the previous seal, or the end of the idle
    /// stretch preceding the interval's first event).
    pub interval_start: Instant,
    /// The semantic event that completed the interval, including its metadata.
    pub boundary: IntervalBoundary,
    /// Foreground work recorded during the interval, in completion order.
    /// [`ForegroundEvent::SmallPolls`] entries are collected into
    /// `small_polls` instead of appearing here.
    pub events: Vec<ForegroundEvent>,
    /// Span-tagged aggregates of task polls below [`TASK_POLL_FLOOR`]. Spans
    /// are retained so occupancy can apportion folded poll time to reporting
    /// windows narrower than the interval.
    pub small_polls: Vec<SmallPollFlush>,
    /// Events lost to the interval's event cap, plus ring losses reported
    /// via [`IntervalSealer::note_lost`].
    pub dropped_events: u64,
    /// Whether the journal had an unobserved gap during this interval.
    pub journal_discontinuous: bool,
}

impl FrameSnapshot {
    /// When the interval ended.
    pub fn interval_end(&self) -> Instant {
        self.boundary.end_time()
    }

    /// The combined count and total of all folded sub-floor polls in the
    /// interval.
    pub fn small_poll_summary(&self) -> PollSummary {
        let mut summary = PollSummary::default();
        for flush in &self.small_polls {
            summary.add(flush.summary);
        }
        summary
    }

    /// Total foreground time occupied within the interval: the union of the
    /// recorded events' spans (clamped to the interval, so nested work like an
    /// action inside an input dispatch is not double counted) plus the folded
    /// small polls. Folded polls cannot be unioned with nested individually
    /// timed work, so this remains a close approximation.
    pub fn occupancy(&self) -> Duration {
        self.occupancy_within(self.interval_start, self.interval_end())
    }

    /// Like [`Self::occupancy`], but measured against an arbitrary window
    /// (e.g. a reporting window anchored at a frame's first invalidation).
    /// Event spans are clamped to the window. Folded polls carry no
    /// individual timestamps, so each flush's total is apportioned by how
    /// much of its span overlaps the window — assuming the folded time is
    /// spread uniformly across the span.
    pub fn occupancy_within(&self, window_start: Instant, window_end: Instant) -> Duration {
        let mut spans: Vec<(Instant, Instant)> = self
            .events
            .iter()
            .map(|event| {
                let start = event.start_time().max(window_start);
                let end = event.end_time().min(window_end).max(start);
                (start, end)
            })
            .collect();
        spans.sort_by_key(|(start, _)| *start);

        let mut occupied = Duration::ZERO;
        let mut merged_until: Option<Instant> = None;
        for (start, end) in spans {
            let start = match merged_until {
                Some(merged_until) => start.max(merged_until),
                None => start,
            };
            occupied += end.duration_since(start);
            merged_until = Some(match merged_until {
                Some(merged_until) => merged_until.max(end),
                None => end,
            });
        }

        let apportioned_small_polls: Duration = self
            .small_polls
            .iter()
            .map(|flush| {
                let span = flush.until.duration_since(flush.since);
                if span.is_zero() {
                    // A point-like flush lies either inside or outside the
                    // window.
                    if flush.since >= window_start && flush.since <= window_end {
                        flush.summary.total
                    } else {
                        Duration::ZERO
                    }
                } else {
                    let overlap_start = flush.since.max(window_start);
                    let overlap_end = flush.until.min(window_end).max(overlap_start);
                    let overlap = overlap_end.duration_since(overlap_start);
                    flush.summary.total.mul_f64(overlap.div_duration_f64(span))
                }
            })
            .sum();
        occupied + apportioned_small_polls
    }

    /// The fraction of the interval the foreground spent working, in `0.0..=1.0`.
    pub fn busy_fraction(&self) -> f64 {
        let interval = self.interval_end().duration_since(self.interval_start);
        if interval.is_zero() {
            return 1.0;
        }
        (self.occupancy().div_duration_f64(interval)).min(1.0)
    }
}

#[derive(Clone)]
pub(crate) struct ForegroundRunnableCounter(Arc<AtomicUsize>);

impl ForegroundRunnableCounter {
    fn new() -> Self {
        Self(Arc::new(AtomicUsize::new(0)))
    }

    pub(crate) fn queued(&self) {
        self.0.fetch_add(1, Ordering::Release);
    }

    fn finished(&self) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                count.checked_sub(1)
            });
    }

    fn has_runnables(&self) -> bool {
        self.0.load(Ordering::Acquire) > 0
    }
}

// Visibility and power changes clear `dirty_at`; `Window::refresh_visibility`
// re-records the frame from the window's actual dirty state afterwards.
struct WindowFrameState {
    visibility: WindowVisibility,
    dirty_at: Option<Instant>,
    visibility_changed_at: Option<Instant>,
}

impl WindowFrameState {
    fn new(visibility: WindowVisibility) -> Self {
        Self {
            visibility,
            dirty_at: None,
            visibility_changed_at: None,
        }
    }
}

struct ForegroundJournalWriter {
    foreground_runnables: ForegroundRunnableCounter,
    publisher: JournalPublisher,
    turn_depth: usize,
    windows: HashMap<WindowId, WindowFrameState>,
    power: PowerState,
    power_changed_at: Option<Instant>,
    read_suspended_time: fn() -> Option<Duration>,
    suspended_time: Option<Duration>,
    retained_since_boundary: bool,
    small_polls: Option<SmallPollFlush>,
}

impl ForegroundJournalWriter {
    fn new(foreground_runnables: ForegroundRunnableCounter, publisher: JournalPublisher) -> Self {
        Self {
            foreground_runnables,
            publisher,
            turn_depth: 0,
            windows: HashMap::new(),
            power: PowerState::Awake,
            power_changed_at: None,
            read_suspended_time: system_suspended_time,
            suspended_time: system_suspended_time(),
            retained_since_boundary: false,
            small_polls: None,
        }
    }

    fn begin_turn(&mut self) {
        self.turn_depth += 1;
    }

    // Idle boundaries require a retained event since the last boundary, not
    // merely folded sub-floor polls. Sporadic wake-ups (timers, file
    // watchers) leave the foreground idle after every tiny poll; a boundary
    // for each would re-admit to the ring the very polls the fold keeps out
    // of it, wrapping the ring within seconds. Folded-only work is discarded
    // at true idle so unrelated wake-ups cannot accumulate toward a later
    // frame budget. Polls remain folded while a frame or runnable is pending.
    fn end_turn(&mut self, ended_at: Instant) {
        let Some(turn_depth) = self.turn_depth.checked_sub(1) else {
            debug_assert!(false, "foreground turn must be begun before it ends");
            return;
        };
        self.turn_depth = turn_depth;
        if self.turn_depth > 0
            || self.foreground_runnables.has_runnables()
            || self.has_unexpired_pending_frame(ended_at)
        {
            return;
        }
        if !self.retained_since_boundary {
            self.small_polls = None;
            return;
        }

        self.record_entry(ForegroundJournalEntry::Boundary(IntervalBoundary::Idle {
            ended_at,
        }));
    }

    fn has_unexpired_pending_frame(&mut self, now: Instant) -> bool {
        for window in self.windows.values_mut() {
            if window
                .dirty_at
                .is_some_and(|at| now.saturating_duration_since(at) >= FRAME_DEADLINE)
            {
                window.dirty_at = None;
            }
        }
        self.power == PowerState::Awake
            && self
                .windows
                .values()
                .any(|window| window.dirty_at.is_some())
    }

    fn power_transition(&mut self, power: PowerState, at: Instant) {
        if self.power == power {
            return;
        }
        self.power = power;
        // A suspend that was notified after it happened must not be reported
        // again by the suspend clock. A notification that precedes its
        // suspend (macOS posts will-sleep up to 30 s early) leaves the clock
        // to fire once more after wake, which only seals an empty interval.
        self.suspended_time = (self.read_suspended_time)();
        self.interrupt_measurements(at);
    }

    /// Interrupts measurements, as a power notification would, if the system
    /// suspended since the previous check. Measurements that end call this
    /// before deciding whether to keep themselves, so a suspend during them
    /// is detected even if its notification hasn't been handled yet.
    //
    // Power notifications arrive on the foreground thread, so work that was
    // running when the system suspended finishes before its notification is
    // handled (and on Windows the notification may never arrive).
    fn detect_suspend(&mut self, at: Instant) {
        let Some(suspended_time) = (self.read_suspended_time)() else {
            return;
        };
        let previous = self.suspended_time.replace(suspended_time);
        if previous
            .is_some_and(|previous| suspended_time.saturating_sub(previous) >= MIN_DETECTED_SUSPEND)
        {
            self.interrupt_measurements(at);
        }
    }

    /// Seals the open interval and invalidates all measurements in progress
    /// at `at`, since they may span a suspend.
    fn interrupt_measurements(&mut self, at: Instant) {
        self.record_entry(ForegroundJournalEntry::Boundary(
            IntervalBoundary::PowerTransition { ended_at: at },
        ));
        self.power_changed_at = Some(at);
        for window in self.windows.values_mut() {
            window.dirty_at = None;
        }
    }

    /// Whether work that started at `start` spans a sleep or a wake, and so
    /// must not be measured as uninterrupted foreground work.
    fn power_interrupted_since(&self, start: Instant) -> bool {
        self.power == PowerState::Suspended || self.power_changed_at.is_some_and(|at| start <= at)
    }

    fn set_visibility(&mut self, id: WindowId, visibility: WindowVisibility, at: Instant) {
        let window = self
            .windows
            .entry(id)
            .or_insert_with(|| WindowFrameState::new(visibility));
        if window.visibility != visibility {
            window.visibility = visibility;
            window.visibility_changed_at = Some(at);
            window.dirty_at = None;
        }
    }

    fn fold_small_poll(&mut self, timing: TaskTiming) {
        let flush = self.small_polls.get_or_insert(SmallPollFlush {
            summary: PollSummary::default(),
            since: timing.start,
            until: timing.end.0,
        });
        flush.summary.count += 1;
        flush.summary.total += timing.poll_duration();
        flush.since = flush.since.min(timing.start);
        flush.until = flush.until.max(timing.end.0);
    }

    fn take_small_polls(&mut self) -> Option<SmallPollFlush> {
        self.small_polls.take()
    }

    fn record_event(&mut self, event: ForegroundEvent) {
        self.retained_since_boundary = true;
        self.record_entry(ForegroundJournalEntry::Event(event));
    }

    fn record_entry(&mut self, entry: ForegroundJournalEntry) {
        let small_polls = self
            .take_small_polls()
            .map(ForegroundEvent::SmallPolls)
            .map(ForegroundJournalEntry::Event);
        self.publisher
            .publish([small_polls, Some(entry)].into_iter().flatten());
        if matches!(entry, ForegroundJournalEntry::Boundary(_)) {
            self.retained_since_boundary = false;
        }
    }

    /// Publishes a point record that is neither work nor a boundary. Folded
    /// polls stay folded: publishing them here would let them reach the
    /// sealer even when [`Self::end_turn`] later discards folded-only work.
    fn record_marker(&mut self, entry: ForegroundJournalEntry) {
        self.publisher.publish([entry]);
    }

    fn record_frame_state(&mut self, change: FrameStateChange) {
        self.record_entry(ForegroundJournalEntry::FrameState(change));
    }

    fn record_platform_signal(&mut self, signal: PlatformSignal) {
        self.record_marker(ForegroundJournalEntry::PlatformSignal(signal));
    }

    /// Seals retained work at a frame request that didn't draw. Like idle
    /// boundaries, skips require a retained event since the last boundary:
    /// platforms that request a frame every refresh would otherwise seal an
    /// empty interval, and flush folded polls, on every idle refresh.
    fn record_frame_skipped(&mut self, skipped: FrameSkipped) {
        if !self.retained_since_boundary {
            return;
        }
        // Even a no-render callback can schedule or invalidate the next frame.
        // Skipping does not establish that the window's pending demand is met.
        self.record_entry(ForegroundJournalEntry::Boundary(
            IntervalBoundary::FrameSkipped(skipped),
        ));
    }

    fn record_frame_pending(&mut self, window_id: WindowId, dirty_at: Instant) {
        let window = self
            .windows
            .entry(window_id)
            .or_insert_with(|| WindowFrameState::new(WindowVisibility::Visible));
        if self.power == PowerState::Suspended
            || !window.visibility.is_visible()
            || window
                .dirty_at
                .is_some_and(|at| dirty_at.saturating_duration_since(at) < FRAME_DEADLINE)
        {
            return;
        }

        window.dirty_at = Some(dirty_at);
        self.record_frame_state(FrameStateChange::Pending {
            window_id,
            dirty_at,
        });
    }

    fn record_window_closed(&mut self, window_id: WindowId, at: Instant) {
        self.windows.remove(&window_id);
        self.record_frame_state(FrameStateChange::Closed { window_id, at });
    }

    fn record_present(&mut self, timing: PresentTiming, frame: Option<FrameTiming>) {
        if let Some(window) = self.windows.get_mut(&timing.window_id) {
            window.dirty_at = None;
        }
        if self.power_interrupted_since(timing.present_start) {
            return;
        }
        match frame {
            Some(frame) => {
                self.record_entry(ForegroundJournalEntry::Boundary(
                    IntervalBoundary::Presented(PresentedFrame {
                        frame,
                        presentation: timing,
                    }),
                ));
            }
            None => {
                self.record_event(ForegroundEvent::Present(timing));
            }
        }
    }
}

thread_local! {
    static FOREGROUND_RUNNABLES: ForegroundRunnableCounter = ForegroundRunnableCounter::new();
    static FOREGROUND_JOURNAL: RefCell<Option<ForegroundJournalWriter>> = const { RefCell::new(None) };
}

pub(crate) fn foreground_runnable_counter() -> ForegroundRunnableCounter {
    FOREGROUND_RUNNABLES.with(Clone::clone)
}

/// Starts journaling on the calling thread. Called once by `App` construction
/// on the main thread; every other thread's recording calls are no-ops.
/// Idempotent so that multiple `App`s on one thread (tests) share one journal.
pub(crate) fn install_foreground_journal() -> ForegroundJournal {
    let foreground_runnables = foreground_runnable_counter();
    FOREGROUND_JOURNAL.with(|journal| {
        let mut journal = journal.borrow_mut();
        if let Some(journal) = journal.as_ref() {
            return ForegroundJournal {
                ring: Arc::clone(&journal.publisher.ring),
            };
        }

        let (handle, publisher) =
            ForegroundJournal::new(MAX_JOURNAL_ENTRIES, MAX_PENDING_JOURNAL_ENTRIES);
        *journal = Some(ForegroundJournalWriter::new(
            foreground_runnables,
            publisher,
        ));
        handle
    })
}

/// Restores the previous foreground journal on drop, so a test's writer
/// (turn depth, pending frames, retained-since-boundary state) never leaks
/// into a later test that happens to reuse the same test-harness thread.
#[cfg(test)]
pub(crate) struct TestForegroundJournalGuard {
    previous: Option<ForegroundJournalWriter>,
    _not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}

#[cfg(test)]
impl Drop for TestForegroundJournalGuard {
    fn drop(&mut self) {
        FOREGROUND_JOURNAL.with(|journal| {
            *journal.borrow_mut() = self.previous.take();
        });
    }
}

/// Installs a fresh journal for the duration of a test, isolated from
/// whatever a previous test left on this thread. Used by tests outside this
/// module (e.g. `bench_context`) that need to observe journal entries
/// without interference from the shared per-thread journal.
#[cfg(test)]
pub(crate) fn install_test_foreground_journal(
    capacity: usize,
    pending_capacity: usize,
) -> (ForegroundJournal, TestForegroundJournalGuard) {
    let foreground_runnables = foreground_runnable_counter();
    let (handle, publisher) = ForegroundJournal::new(capacity, pending_capacity);
    let previous = FOREGROUND_JOURNAL.with(|journal| {
        journal.borrow_mut().replace(
            ForegroundJournalWriter::new(foreground_runnables, publisher).with_test_suspend_clock(),
        )
    });
    (
        handle,
        TestForegroundJournalGuard {
            previous,
            _not_send: std::marker::PhantomData,
        },
    )
}

#[cfg(test)]
thread_local! {
    static TEST_SUSPENDED_TIME: std::cell::Cell<Duration> = const { std::cell::Cell::new(Duration::ZERO) };
}

#[cfg(test)]
fn test_suspended_time() -> Option<Duration> {
    Some(TEST_SUSPENDED_TIME.get())
}

/// Simulates the system spending `duration` suspended, as observed through
/// the suspend clock of journals using [`ForegroundJournalWriter::with_test_suspend_clock`].
#[cfg(test)]
pub(crate) fn advance_test_suspended_time(duration: Duration) {
    TEST_SUSPENDED_TIME.set(TEST_SUSPENDED_TIME.get() + duration);
}

#[cfg(test)]
impl ForegroundJournalWriter {
    /// Replaces the system suspend clock, so a real suspend during a test
    /// can't interrupt its measurements.
    fn with_test_suspend_clock(mut self) -> Self {
        TEST_SUSPENDED_TIME.set(Duration::ZERO);
        self.read_suspended_time = test_suspended_time;
        self.suspended_time = test_suspended_time();
        self
    }
}

fn with_journal(f: impl FnOnce(&mut ForegroundJournalWriter)) {
    FOREGROUND_JOURNAL.with(|journal| {
        if let Some(journal) = journal.borrow_mut().as_mut() {
            f(journal);
        }
    });
}

pub(crate) fn observe_power(cx: &App) {
    cx.on_system_sleep(|_| {
        record_power_transition(PowerState::Suspended);
    })
    .detach();
    // Visibility notifications delivered during sleep may have been missed.
    cx.on_system_wake(|cx| {
        record_power_transition(PowerState::Awake);
        cx.spawn(async |cx| {
            cx.update(|cx| {
                for handle in cx.windows() {
                    handle
                        .update(cx, |_, window, cx| window.refresh_visibility(cx))
                        .log_err();
                }
            });
        })
        .detach();
    })
    .detach();
}

/// Total time the system has spent suspended since boot, or `None` where no
/// clock provides it: the difference between a clock that counts suspended
/// time and one that doesn't. The suspend-excluding clock is read first so
/// read latency can't make the difference negative. Every read is served from
/// memory the kernel maps into the process, without a syscall.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn system_suspended_time() -> Option<Duration> {
    // CLOCK_BOOTTIME is CLOCK_MONOTONIC plus time spent suspended:
    // https://man7.org/linux/man-pages/man2/clock_gettime.2.html
    let awake = read_clock(libc::CLOCK_MONOTONIC)?;
    let total = read_clock(libc::CLOCK_BOOTTIME)?;
    Some(total.saturating_sub(awake))
}

#[cfg(target_os = "macos")]
fn system_suspended_time() -> Option<Duration> {
    use std::sync::OnceLock;

    // Declared here because libc deprecates its mach bindings, and these
    // three functions don't justify a `mach2` dependency. `<mach/mach_time.h>`
    // documents `mach_continuous_time` as "like mach_absolute_time, but
    // advances during sleep", so both count the same ticks:
    // https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/mach_time.h
    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
        fn mach_continuous_time() -> u64;
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
    }

    static TIMEBASE: OnceLock<Option<(u32, u32)>> = OnceLock::new();
    let (numer, denom) = (*TIMEBASE.get_or_init(|| {
        let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: `info` is a valid, writable mach_timebase_info.
        let succeeded = unsafe { mach_timebase_info(&mut info) } == 0;
        (succeeded && info.denom != 0).then_some((info.numer, info.denom))
    }))?;
    // SAFETY: neither function has preconditions.
    let awake = unsafe { mach_absolute_time() };
    // SAFETY: as above.
    let total = unsafe { mach_continuous_time() };
    let nanos = u128::from(total.saturating_sub(awake)) * u128::from(numer) / u128::from(denom);
    Some(Duration::from_nanos(u64::try_from(nanos).ok()?))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn read_clock(clock: libc::clockid_t) -> Option<Duration> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `time` is a valid, writable timespec.
    if unsafe { libc::clock_gettime(clock, &mut time) } != 0 {
        return None;
    }
    Some(Duration::new(
        u64::try_from(time.tv_sec).ok()?,
        u32::try_from(time.tv_nsec).ok()?,
    ))
}

#[cfg(target_os = "windows")]
fn system_suspended_time() -> Option<Duration> {
    use windows::Win32::System::WindowsProgramming::{
        QueryInterruptTime, QueryUnbiasedInterruptTime,
    };
    let mut awake = 0;
    // SAFETY: `awake` is a valid, writable u64.
    if !unsafe { QueryUnbiasedInterruptTime(&mut awake) }.as_bool() {
        return None;
    }
    // SAFETY: no preconditions.
    let total = unsafe { QueryInterruptTime() };
    // Both count 100 ns units; unbiased interrupt time excludes sleep and
    // hibernation:
    // https://learn.microsoft.com/en-us/windows/win32/sysinfo/interrupt-time
    Some(Duration::from_nanos(
        total.saturating_sub(awake).saturating_mul(100),
    ))
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "windows"
)))]
fn system_suspended_time() -> Option<Duration> {
    None
}

pub(crate) fn record_power_transition(state: PowerState) {
    with_journal(|journal| journal.power_transition(state, Instant::now()));
}

/// Whether work that started at `start` spans a sleep or a wake. Such work
/// is dropped rather than measured.
pub(crate) fn power_interrupted_since(start: Instant) -> bool {
    let mut interrupted = false;
    with_journal(|journal| {
        journal.detect_suspend(Instant::now());
        interrupted = journal.power_interrupted_since(start);
    });
    interrupted
}

pub(crate) fn record_window_visibility(id: WindowId, visibility: WindowVisibility) {
    with_journal(|journal| journal.set_visibility(id, visibility, Instant::now()));
}

/// Whether a frame sample that started at `start` was measured while the
/// window stayed visible and the system stayed awake.
pub(crate) fn frame_sample_is_valid(id: WindowId, start: Instant) -> bool {
    let mut valid = true;
    with_journal(|journal| {
        journal.detect_suspend(Instant::now());
        valid = !journal.power_interrupted_since(start)
            && journal.windows.get(&id).is_none_or(|window| {
                window.visibility.is_visible()
                    && window.visibility_changed_at.is_none_or(|at| start > at)
            });
    });
    valid
}

// TODO(gpui-profiler): the turn brackets in the dispatchers and
// WindowProfiler are bare begin/end call pairs rather than uses of this
// guard. A caught unwind between a pair leaves `turn_depth` (and potentially
// the runnable counter) permanently unbalanced, which silently disables
// idle boundaries for the rest of the process. Zed aborts on panics so
// this is latent today, but gpui-as-a-library callers may catch unwinds;
// route all bracketing through this guard.
pub(crate) struct ForegroundTurnGuard;

impl Drop for ForegroundTurnGuard {
    fn drop(&mut self) {
        end_foreground_turn();
    }
}

pub(crate) fn foreground_turn() -> ForegroundTurnGuard {
    begin_foreground_turn();
    ForegroundTurnGuard
}

pub(crate) fn begin_foreground_turn() {
    // Observing before the turn's work takes its start time means a suspend
    // detected here never invalidates the work that follows it.
    with_journal(|journal| {
        journal.detect_suspend(Instant::now());
        journal.begin_turn();
    });
}

pub(crate) fn end_foreground_turn() {
    with_journal(|journal| journal.end_turn(Instant::now()));
}

pub(crate) fn record_task_poll(timing: TaskTiming) {
    FOREGROUND_RUNNABLES.with(ForegroundRunnableCounter::finished);
    with_journal(|journal| {
        journal.detect_suspend(timing.end.0);
        if journal.power_interrupted_since(timing.start) {
            journal.end_turn(timing.end.0);
            return;
        }
        if timing.poll_duration() >= TASK_POLL_FLOOR {
            journal.record_event(ForegroundEvent::TaskPoll(timing));
        } else {
            journal.fold_small_poll(timing);
        }
        journal.end_turn(timing.end.0);
    });
}

pub(crate) fn record_action(timing: ActionTiming) {
    with_journal(|journal| journal.record_event(ForegroundEvent::Action(timing)));
}

pub(crate) fn record_input(timing: InputTiming) {
    with_journal(|journal| journal.record_event(ForegroundEvent::Input(timing)));
}

pub(crate) fn record_view(timing: ViewTiming) {
    with_journal(|journal| journal.record_event(ForegroundEvent::View(timing)));
}

pub(crate) fn record_draw(timing: FrameTiming) {
    with_journal(|journal| journal.record_event(ForegroundEvent::Draw(timing)));
}

pub(crate) fn record_present(timing: PresentTiming, frame: Option<FrameTiming>) {
    with_journal(|journal| journal.record_present(timing, frame));
}

pub(crate) fn record_platform_signal(signal: PlatformSignal) {
    with_journal(|journal| journal.record_platform_signal(signal));
}

pub(crate) fn record_reentrant_frame_skipped(window_id: WindowId, at: Instant) {
    with_journal(|journal| {
        journal.record_marker(ForegroundJournalEntry::ReentrantFrameSkipped { window_id, at });
    });
}

pub(crate) fn record_frame_skipped(window_id: WindowId, at: Instant, reason: FrameSkipReason) {
    with_journal(|journal| {
        journal.record_frame_skipped(FrameSkipped {
            window_id,
            at,
            reason,
        })
    });
}

pub(crate) fn record_frame_pending(window_id: WindowId, dirty_at: Instant) {
    with_journal(|journal| journal.record_frame_pending(window_id, dirty_at));
}

pub(crate) fn record_window_closed(window_id: WindowId) {
    let at = Instant::now();
    with_journal(|journal| journal.record_window_closed(window_id, at));
}

const SLOT_WRITER: usize = 1 << (usize::BITS - 1);
const SLOT_READER_MASK: usize = !SLOT_WRITER;
const EMPTY_SEQUENCE: u64 = u64::MAX;

struct JournalSlot {
    users: AtomicUsize,
    sequence: AtomicU64,
    entry: UnsafeCell<MaybeUninit<ForegroundJournalEntry>>,
}

// `users` ensures the entry is only read while no writer owns the slot and is
// only written while no readers own it.
unsafe impl Sync for JournalSlot {}

impl JournalSlot {
    fn new() -> Self {
        Self {
            users: AtomicUsize::new(0),
            sequence: AtomicU64::new(EMPTY_SEQUENCE),
            entry: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    fn try_publish(&self, sequence: u64, entry: ForegroundJournalEntry) -> bool {
        if self
            .users
            .compare_exchange(0, SLOT_WRITER, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            return false;
        }

        // SAFETY: setting SLOT_WRITER from zero gives this writer exclusive
        // access to the slot until the Release store below.
        unsafe {
            (*self.entry.get()).write(entry);
        }
        self.sequence.store(sequence, Ordering::Relaxed);
        self.users.store(0, Ordering::Release);
        true
    }

    fn try_read(&self, expected_sequence: u64) -> Option<ForegroundJournalEntry> {
        let _guard = JournalSlotReadGuard::try_new(self)?;
        if self.sequence.load(Ordering::Relaxed) != expected_sequence {
            return None;
        }

        // SAFETY: a matching sequence means the slot was initialized for this
        // logical entry, and `guard` prevents the writer from overwriting it
        // until after the Copy.
        Some(unsafe { *(*self.entry.get()).assume_init_ref() })
    }

    fn try_add_reader(&self) -> bool {
        let mut users = self.users.load(Ordering::Relaxed);
        loop {
            if users & SLOT_WRITER != 0 || users == SLOT_READER_MASK {
                return false;
            }
            match self.users.compare_exchange_weak(
                users,
                users + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(updated_users) => users = updated_users,
            }
        }
    }

    fn remove_reader(&self) {
        let previous = self.users.fetch_sub(1, Ordering::Release);
        debug_assert!(
            previous > 0 && previous & SLOT_WRITER == 0,
            "invalid slot reader state: {previous:#x}"
        );
    }
}

struct JournalSlotReadGuard<'a> {
    slot: &'a JournalSlot,
}

impl<'a> JournalSlotReadGuard<'a> {
    fn try_new(slot: &'a JournalSlot) -> Option<Self> {
        if slot.try_add_reader() {
            Some(Self { slot })
        } else {
            None
        }
    }
}

impl Drop for JournalSlotReadGuard<'_> {
    fn drop(&mut self) {
        self.slot.remove_reader();
    }
}

struct JournalRing {
    slots: Box<[JournalSlot]>,
    finalized: AtomicU64,
    offered: AtomicU64,
}

impl JournalRing {
    fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            slots: (0..capacity).map(|_| JournalSlot::new()).collect(),
            finalized: AtomicU64::new(0),
            offered: AtomicU64::new(0),
        }
    }

    fn capacity(&self) -> usize {
        self.slots.len()
    }

    fn try_publish(&self, sequence: u64, entry: ForegroundJournalEntry) -> bool {
        let index = (sequence % self.slots.len() as u64) as usize;
        self.slots[index].try_publish(sequence, entry)
    }

    fn read(&self, sequence: u64) -> Option<ForegroundJournalEntry> {
        let index = (sequence % self.slots.len() as u64) as usize;
        self.slots[index].try_read(sequence)
    }
}

struct PendingJournalEntry {
    sequence: u64,
    entry: ForegroundJournalEntry,
}

struct JournalPublisher {
    ring: Arc<JournalRing>,
    next_sequence: u64,
    pending: VecDeque<PendingJournalEntry>,
    dropped_after_pending: u64,
    pending_capacity: usize,
}

impl JournalPublisher {
    fn new(ring: Arc<JournalRing>, pending_capacity: usize) -> Self {
        Self {
            ring,
            next_sequence: 0,
            pending: VecDeque::with_capacity(pending_capacity),
            dropped_after_pending: 0,
            pending_capacity,
        }
    }

    fn publish(&mut self, entries: impl IntoIterator<Item = ForegroundJournalEntry>) {
        for entry in entries {
            self.flush_pending();
            self.publish_one(entry);
        }
        self.flush_pending();
    }

    fn publish_one(&mut self, entry: ForegroundJournalEntry) {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.ring
            .offered
            .store(self.next_sequence, Ordering::Release);

        if !self.pending.is_empty() || self.dropped_after_pending > 0 {
            if self.dropped_after_pending == 0 && self.pending.len() < self.pending_capacity {
                self.pending
                    .push_back(PendingJournalEntry { sequence, entry });
            } else {
                self.dropped_after_pending += 1;
            }
            return;
        }

        if self.ring.try_publish(sequence, entry) {
            self.ring.finalized.store(sequence + 1, Ordering::Release);
        } else if self.pending_capacity > 0 {
            self.pending
                .push_back(PendingJournalEntry { sequence, entry });
        } else {
            self.dropped_after_pending = 1;
        }
    }

    fn flush_pending(&mut self) {
        while let Some(pending) = self.pending.front() {
            if !self.ring.try_publish(pending.sequence, pending.entry) {
                return;
            }
            let sequence = pending.sequence;
            self.pending.pop_front();
            self.ring.finalized.store(sequence + 1, Ordering::Release);
        }

        if self.dropped_after_pending > 0 {
            self.ring
                .finalized
                .store(self.next_sequence, Ordering::Release);
            self.dropped_after_pending = 0;
        }
    }
}

/// A cloneable handle to one foreground journal stream.
///
/// Each collector has an independent cursor. Collectors briefly pin one slot
/// at a time and never block recording; entries they could not observe are
/// reported as discontinuities. Apps on the same foreground thread share the
/// stream.
#[derive(Clone)]
pub struct ForegroundJournal {
    ring: Arc<JournalRing>,
}

impl ForegroundJournal {
    fn new(capacity: usize, pending_capacity: usize) -> (Self, JournalPublisher) {
        let ring = Arc::new(JournalRing::new(capacity));
        (
            Self {
                ring: Arc::clone(&ring),
            },
            JournalPublisher::new(ring, pending_capacity),
        )
    }

    /// Creates an independent collector that observes entries offered after
    /// this call.
    pub fn collector(&self) -> ForegroundJournalCollector {
        ForegroundJournalCollector {
            cursor: self.ring.offered.load(Ordering::Acquire),
            ring: Arc::clone(&self.ring),
        }
    }
}

/// Entries returned by one [`ForegroundJournalCollector::collect_unseen`] call.
#[derive(Debug, Default)]
pub struct DrainedEntries {
    /// Journal entries recorded since the previous drain, in recording order,
    /// including synthetic [`ForegroundJournalEntry::Discontinuity`] markers at
    /// unavailable logical positions.
    pub entries: Vec<ForegroundJournalEntry>,
    /// Entries unavailable to this collector because they were overwritten or
    /// could not be published after the fixed pending queue filled. This is the
    /// aggregate of the discontinuity markers in `entries`.
    pub lost: u64,
}

/// Reads the foreground stream, tracking a cursor so each call to
/// [`Self::collect_unseen`] returns only entries recorded since the previous
/// call. Independent collectors do not affect each other.
pub struct ForegroundJournalCollector {
    cursor: u64,
    ring: Arc<JournalRing>,
}

impl ForegroundJournalCollector {
    /// Returns entries recorded since the previous call (or since the
    /// collector was created), reporting how many were unavailable before this
    /// drain observed them.
    pub fn collect_unseen(&mut self) -> DrainedEntries {
        let end = self.ring.finalized.load(Ordering::Acquire);
        if self.cursor >= end {
            return DrainedEntries::default();
        }
        let retained_start = end.saturating_sub(self.ring.capacity() as u64);
        let mut lost = retained_start.saturating_sub(self.cursor);
        self.cursor = self.cursor.max(retained_start);
        let mut entries = Vec::with_capacity((end - self.cursor) as usize + usize::from(lost > 0));
        if lost > 0 {
            entries.push(ForegroundJournalEntry::Discontinuity { lost });
        }

        while self.cursor < end {
            if let Some(entry) = self.ring.read(self.cursor) {
                entries.push(entry);
            } else {
                lost += 1;
                match entries.last_mut() {
                    Some(ForegroundJournalEntry::Discontinuity { lost }) => *lost += 1,
                    _ => entries.push(ForegroundJournalEntry::Discontinuity { lost: 1 }),
                }
            }
            self.cursor += 1;
        }

        DrainedEntries { entries, lost }
    }
}

/// A pure state machine that groups foreground journal entries into
/// [`FrameSnapshot`]s.
///
/// Feed it drained entries in recording order. Work is carried across calls
/// until an explicit [`IntervalBoundary`] arrives. Idle time before an
/// interval's first event is excluded without relying on an elapsed-time
/// heuristic.
#[derive(Debug)]
pub struct IntervalSealer {
    interval_start: Instant,
    events: Vec<ForegroundEvent>,
    small_polls: Vec<SmallPollFlush>,
    dropped_events: u64,
    journal_discontinuous: bool,
}

impl IntervalSealer {
    /// Creates a sealer whose first interval starts at `start` (typically
    /// the moment the consumer's collector was created).
    pub fn new(start: Instant) -> Self {
        Self {
            interval_start: start,
            events: Vec::new(),
            small_polls: Vec::new(),
            dropped_events: 0,
            journal_discontinuous: false,
        }
    }

    /// Accounts for a loss supplied outside a collector drain. Collector losses
    /// are already represented by ordered discontinuity entries.
    pub fn note_lost(&mut self, lost: u64) {
        self.dropped_events += lost;
        self.journal_discontinuous |= lost > 0;
    }

    /// Processes a batch of drained entries, returning the snapshots completed
    /// by explicit boundaries. Work without a following boundary is carried
    /// over to subsequent calls.
    pub fn push_entries(
        &mut self,
        entries: impl IntoIterator<Item = ForegroundJournalEntry>,
    ) -> Vec<FrameSnapshot> {
        let mut snapshots = Vec::new();
        for entry in entries {
            match entry {
                ForegroundJournalEntry::Event(event) => {
                    if self.is_empty() {
                        self.interval_start = self.interval_start.max(event.start_time());
                    }
                    match event {
                        ForegroundEvent::SmallPolls(flush) => self.push_small_polls(flush),
                        event => self.push_event(event),
                    }
                }
                ForegroundJournalEntry::Boundary(boundary) => {
                    if let IntervalBoundary::Presented(presented) = boundary {
                        if self.is_empty() {
                            self.interval_start = self
                                .interval_start
                                .max(presented.presentation.present_start);
                        }
                        self.push_event(ForegroundEvent::Present(presented.presentation));
                    }
                    if self.is_empty() {
                        self.interval_start = self.interval_start.max(boundary.end_time());
                    } else {
                        snapshots.push(self.seal(boundary));
                    }
                }
                // Pending-frame state gates boundaries on the writer side;
                // the entries remain in the stream for consumers that want
                // dirty timing, but the sealer has no use for them.
                ForegroundJournalEntry::FrameState(_)
                | ForegroundJournalEntry::PlatformSignal(_)
                | ForegroundJournalEntry::ReentrantFrameSkipped { .. } => {}
                ForegroundJournalEntry::Discontinuity { lost } => self.note_lost(lost),
            }
        }
        snapshots
    }

    fn is_empty(&self) -> bool {
        self.events.is_empty()
            && self.small_polls.is_empty()
            && self.dropped_events == 0
            && !self.journal_discontinuous
    }

    fn push_event(&mut self, event: ForegroundEvent) {
        if self.events.len() >= MAX_INTERVAL_EVENTS {
            self.dropped_events += 1;
        } else {
            self.events.push(event);
        }
    }

    fn push_small_polls(&mut self, flush: SmallPollFlush) {
        if self.small_polls.len() >= MAX_INTERVAL_EVENTS
            && let Some(last) = self.small_polls.last_mut()
        {
            // Degrade gracefully at the cap: widen the last flush's span
            // instead of dropping poll time, at the cost of coarser
            // apportioning.
            last.summary.add(flush.summary);
            last.since = last.since.min(flush.since);
            last.until = last.until.max(flush.until);
        } else {
            self.small_polls.push(flush);
        }
    }

    fn seal(&mut self, boundary: IntervalBoundary) -> FrameSnapshot {
        let ended = boundary.end_time();
        let snapshot = FrameSnapshot {
            interval_start: self.interval_start,
            boundary,
            events: std::mem::take(&mut self.events),
            small_polls: std::mem::take(&mut self.small_polls),
            dropped_events: std::mem::take(&mut self.dropped_events),
            journal_discontinuous: std::mem::take(&mut self.journal_discontinuous),
        };
        self.interval_start = ended;
        snapshot
    }
}

#[cfg(test)]
mod tests;
