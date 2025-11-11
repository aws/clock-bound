//! Flavours of clock sync algorithms
//!
//! This is where math happens

pub mod event_buffer;

mod ntp;
pub use ntp::Ntp;

mod phc;
pub use phc::Phc;

mod uncorrected_clock;
pub use uncorrected_clock::UncorrectedClock;

use crate::daemon::time::{Duration, tsc::Period};

/// Used as the output for `calculate_local_period_and_error` methods
struct LocalPeriodAndError {
    /// period calculation
    period_local: Period,
    /// period error
    error: Period,
}

/// Output from `calculate_theta` methods
struct CalculateThetaOutput {
    /// The time correction to be applied
    theta: Duration,
    /// The worst clock error bound used in calculation
    clock_error_bound: Duration,
}
