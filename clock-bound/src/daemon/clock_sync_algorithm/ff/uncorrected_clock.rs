//! A naive uncorrected clock

use crate::daemon::time::{Instant, TscCount, tsc::Period};

/// A naive uncorrected clock
///
/// `C_u(t) = TSC(t) * p_estimate + K`, where
/// - `C_u(t)` is the uncorrected clock
/// - `TSC(t)` is the TSC value at time t
/// - `p_estimate` is the estimated clock period
/// - `K` is a constant offset of the system epoch. In other words, the calculated time at TSC(0)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UncorrectedClock {
    pub p_estimate: Period,
    pub k: Instant,
}

impl UncorrectedClock {
    /// Calculate the uncorrected time of a corresponding TSC value
    pub fn time_at(&self, tsc: TscCount) -> Instant {
        tsc.uncorrected_time(self.p_estimate, self.k)
    }
}
