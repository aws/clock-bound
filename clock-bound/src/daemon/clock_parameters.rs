//! Calculated clock parameters
//!
//! The output of the [`ClockSyncAlgorithm`](super::clock_sync_algorithm)

use crate::daemon::time::{Duration, Instant, TscCount, tsc::Period};

/// Clock parameters
///
/// These values are calculated by the [`ClockSyncAlgorithm`](super::clock_sync_algorithm)
/// and used by the [`ClockState`](super::clock_state)
#[derive(Debug, Clone, PartialEq)]
pub struct ClockParameters {
    /// The tsc values that these account for
    pub tsc_count: TscCount,
    /// The time at `tsc_count`
    pub time: Instant,
    /// The clock error bound of `time` at `tsc_count`
    pub clock_error_bound: Duration,
    /// The period of the TSC clock at `tsc_count`
    pub period: Period,
    /// The max error of the `period` at `tsc_count`
    pub period_max_error: Period,
}

/// Information on the selected clock
///
/// This struct is stored in the [`ClockSyncAlgorithm`](super::clock_sync_algorithm) as the
/// final output product of the algorithm.
///
/// Includes [`ClockParameters`] as well as information about the
/// selected clock
///
/// TODO: Include the name of the source, as well.
pub struct SelectedClockInfo {
    /// Calculated clock parameters from the [`ClockSyncAlgorithm`](super::clock_sync_algorithm)
    pub clock_parameters: ClockParameters,
    /// Stratum of the selected clock
    ///
    /// None if reading from a non-NTP device
    pub stratum: Option<u8>, // TODO: use the enum in another PR
}
