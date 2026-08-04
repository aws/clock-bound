//! Core types for the SHM validation test.
//!
//! Defines data structures for capturing and evaluating ClockBound error bound measurements.

use clock_bound::shm::{ClockBoundNowResult, ClockStatus};
use nix::sys::time::{TimeSpec, TimeValLike};
use serde::Serialize;

/// Raw measurement captured in the hot loop — no computation, just data.
/// All validation is deferred to `BoundCheck::from_sample`.
pub struct RawSample {
    pub elapsed_secs: f64,
    pub realtime: TimeSpec,
    pub before: ClockBoundNowResult,
    pub after: ClockBoundNowResult,
}

/// A single ClockBound SHM read (before or after the clock read).
#[derive(Debug, Clone, Serialize)]
pub struct BoundWindow {
    pub earliest_ns: i64,
    pub latest_ns: i64,
    pub clock_status: String,
}

impl BoundWindow {
    pub fn new(cb_result: &ClockBoundNowResult) -> Self {
        Self {
            earliest_ns: cb_result.earliest.num_nanoseconds(),
            latest_ns: cb_result.latest.num_nanoseconds(),
            clock_status: format!("{:?}", cb_result.clock_status),
        }
    }

    /// Combine into a validation envelope: before.earliest to after.latest.
    pub fn combine(before: &BoundWindow, after: &BoundWindow) -> Self {
        Self {
            earliest_ns: before.earliest_ns,
            latest_ns: after.latest_ns,
            clock_status: after.clock_status.clone(),
        }
    }

    pub fn contains(&self, realtime_ns: i64) -> bool {
        realtime_ns >= self.earliest_ns && realtime_ns <= self.latest_ns
    }
}

/// Full result of a single bound check iteration.
///
/// Containment results are derived via methods from the window data.
#[derive(Debug, Clone, Serialize)]
pub struct BoundCheck {
    pub elapsed_secs: f64,
    pub realtime_ns: i64,
    pub before: BoundWindow,
    pub after: BoundWindow,
    pub combined: BoundWindow,
    pub invalid_clockstate: bool,
}

impl BoundCheck {
    pub fn from_sample(sample: &RawSample) -> Self {
        let realtime_ns = sample.realtime.num_nanoseconds();
        let before = BoundWindow::new(&sample.before);
        let after = BoundWindow::new(&sample.after);
        let combined = BoundWindow::combine(&before, &after);

        let invalid_clockstate = !matches!(
            (&sample.before.clock_status, &sample.after.clock_status),
            (
                ClockStatus::Synchronized | ClockStatus::FreeRunning,
                ClockStatus::Synchronized | ClockStatus::FreeRunning
            )
        );

        Self {
            elapsed_secs: sample.elapsed_secs,
            realtime_ns,
            before,
            after,
            combined,
            invalid_clockstate,
        }
    }

    pub fn before_contains_realtime(&self) -> bool {
        self.before.contains(self.realtime_ns)
    }

    pub fn after_contains_realtime(&self) -> bool {
        self.after.contains(self.realtime_ns)
    }

    /// Primary assertion: realtime is within the combined envelope.
    pub fn combined_contains_realtime(&self) -> bool {
        self.combined.contains(self.realtime_ns)
    }
}
