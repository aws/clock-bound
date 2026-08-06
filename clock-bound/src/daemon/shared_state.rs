//! Observable shared state for daemon logging and monitoring.
//!
//! These types wrap internal state in mutex-guarded newtypes so that callers
//! only interact through getter/setter methods — ensuring the lock is never
//! held longer than a single operation.

use std::sync::{Arc, Mutex};

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::SourceInfo;
use crate::shm::ClockStatus;

/// The currently selected clock source and its parameters.
///
/// Written by the [`Selector`](crate::daemon::clock_sync_algorithm::Selector)
/// whenever it accepts a new measurement.
#[derive(Debug, Clone)]
pub struct SelectedClock {
    clock_parameters: ClockParameters,
    source_info: SourceInfo,
}

impl SelectedClock {
    /// Create a new `SelectedClock` from its components.
    pub fn new(clock_parameters: ClockParameters, source_info: SourceInfo) -> Self {
        Self {
            clock_parameters,
            source_info,
        }
    }

    /// Get the clock parameters.
    pub fn clock_parameters(&self) -> &ClockParameters {
        &self.clock_parameters
    }

    /// Get the source info.
    pub fn source_info(&self) -> &SourceInfo {
        &self.source_info
    }
}

/// Shared access to the currently selected clock.
///
/// Only intended for logging purposes.
#[derive(Debug, Clone)]
pub struct SharedSelectedClock(Arc<Mutex<Option<SelectedClock>>>);

impl SharedSelectedClock {
    /// Create a new empty `SharedSelectedClock`.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }

    /// Get a clone of the current selected clock, if any.
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    pub fn selected_clock(&self) -> Option<SelectedClock> {
        self.0.lock().unwrap().clone()
    }

    /// Set the selected clock value.
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    pub fn set(&self, value: SelectedClock) {
        *self.0.lock().unwrap() = Some(value);
    }

    /// Clear the selected clock (e.g. on disruption).
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    pub fn clear(&self) {
        *self.0.lock().unwrap() = None;
    }
}

impl Default for SharedSelectedClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared access to the current clock synchronization status.
///
/// Written by [`ClockState`](crate::daemon::clock_state::ClockState) on status transitions.
/// Only intended for logging purposes.
#[derive(Debug, Clone)]
pub struct SharedClockStatus(Arc<Mutex<ClockStatus>>);

impl SharedClockStatus {
    /// Create a new `SharedClockStatus` with the given initial status.
    pub fn new(status: ClockStatus) -> Self {
        Self(Arc::new(Mutex::new(status)))
    }

    /// Get the current clock status.
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    pub fn clock_status(&self) -> ClockStatus {
        *self.0.lock().unwrap()
    }

    /// Set the clock status.
    ///
    /// # Panics
    /// Panics if the mutex is poisoned.
    pub fn set(&self, status: ClockStatus) {
        *self.0.lock().unwrap() = status;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::time::{Duration, Instant, TscCount, tsc::Period};

    fn sample_clock_parameters() -> ClockParameters {
        ClockParameters {
            tsc_count: TscCount::new(1_000_000),
            time: Instant::new(1_000_000_000),
            clock_error_bound: Duration::from_nanos(500),
            period: Period::from_seconds(1e-9),
            period_max_error: Period::from_seconds(1e-11),
            as_of_monotonic: Instant::new(0),
        }
    }

    #[test]
    fn shared_selected_clock_starts_empty() {
        let shared = SharedSelectedClock::new();
        assert!(shared.selected_clock().is_none());
    }

    #[test]
    fn shared_selected_clock_set_and_read() {
        let shared = SharedSelectedClock::new();
        let sc = SelectedClock::new(sample_clock_parameters(), SourceInfo::Phc);

        shared.set(sc);

        let read = shared.selected_clock().unwrap();
        assert_eq!(read.clock_parameters().tsc_count, TscCount::new(1_000_000));
        assert_eq!(*read.source_info(), SourceInfo::Phc);
    }

    #[test]
    fn shared_selected_clock_clear() {
        let shared = SharedSelectedClock::new();
        shared.set(SelectedClock::new(
            sample_clock_parameters(),
            SourceInfo::Phc,
        ));

        shared.clear();

        assert!(shared.selected_clock().is_none());
    }

    #[test]
    fn shared_clock_status_initial_value() {
        let shared = SharedClockStatus::new(ClockStatus::Unknown);
        assert_eq!(shared.clock_status(), ClockStatus::Unknown);
    }

    #[test]
    fn shared_clock_status_set_and_read() {
        let shared = SharedClockStatus::new(ClockStatus::Unknown);

        shared.set(ClockStatus::Synchronized);

        assert_eq!(shared.clock_status(), ClockStatus::Synchronized);
    }

    #[test]
    fn shared_clock_status_multiple_transitions() {
        let shared = SharedClockStatus::new(ClockStatus::Unknown);

        shared.set(ClockStatus::Synchronized);
        shared.set(ClockStatus::FreeRunning);

        assert_eq!(shared.clock_status(), ClockStatus::FreeRunning);
    }

    #[test]
    fn selected_clock_getters() {
        let params = sample_clock_parameters();
        let sc = SelectedClock::new(params.clone(), SourceInfo::Phc);

        assert_eq!(sc.clock_parameters().tsc_count, params.tsc_count);
        assert_eq!(
            sc.clock_parameters().clock_error_bound,
            params.clock_error_bound
        );
        assert_eq!(*sc.source_info(), SourceInfo::Phc);
    }
}
