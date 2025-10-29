//! A naive uncorrected clock

use crate::daemon::time::{Instant, tsc::Period};

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
