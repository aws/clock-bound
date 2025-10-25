//! Clock synchronization events
//!
//! These are the in-memory representations of NTP and PHC reads.
mod ntp;
pub use ntp::{Ntp, NtpData, Stratum, TryFromU8Error, ValidStratumLevel};

use crate::daemon::time::{TscCount, TscDiff};

/// A time synchronization event handled by ClockBound
pub enum Event {
    /// NTP Event
    Ntp(Ntp),
}

/// Simple abstraction around types that have a TSC read before and after reference clock reads
pub trait TscRtt {
    /// The TSC read before sending an event request
    fn tsc_pre(&self) -> TscCount;

    /// The TSC read after receiving an event response
    fn tsc_post(&self) -> TscCount;

    /// The TSC round-trip-time of an event
    fn rtt(&self) -> TscDiff {
        self.tsc_post() - self.tsc_pre()
    }

    /// The TSC midpoint
    fn tsc_midpoint(&self) -> TscCount {
        self.tsc_pre().midpoint(self.tsc_post())
    }
}
