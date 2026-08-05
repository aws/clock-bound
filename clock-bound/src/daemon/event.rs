//! Clock synchronization events
//!
//! These are the in-memory representations of NTP and PHC reads.
mod ntp;
pub use ntp::{Ntp, NtpData, Stratum, TryFromU8Error, ValidStratumLevel};

mod phc;
pub use phc::{Phc, PhcData};

use crate::daemon::time::{TscCount, TscDiff};
#[cfg(feature = "daemon")]
use {
    crate::daemon::io::tsc::{read_timestamp_counter_begin, read_timestamp_counter_end},
    crate::daemon::time::{Clock, clocks::RealTime},
};

/// A time synchronization event handled by ClockBound
pub enum Event {
    /// NTP Event
    Ntp(Ntp),
    /// PHC Event
    Phc(Phc),
}

/// Simple abstraction around types that have a TSC read before and after reference clock reads
pub trait TscRtt {
    /// The TSC read before sending an event request
    fn counter_pre(&self) -> TscCount;

    /// The TSC read after receiving an event response
    fn counter_post(&self) -> TscCount;

    /// The TSC round-trip-time of an event
    fn rtt(&self) -> TscDiff {
        self.counter_post() - self.counter_pre()
    }

    /// The TSC midpoint
    fn tsc_midpoint(&self) -> TscCount {
        self.counter_pre().midpoint(self.counter_post())
    }
}

/// Struct containing a system clock read and a TSC read
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemClockMeasurement {
    /// The system clock read
    pub system_time: crate::daemon::time::Instant,
    /// The TSC read
    pub tsc: TscCount,
}

impl SystemClockMeasurement {
    /// Create a new [`SystemClockMeasurement`]
    #[cfg(feature = "daemon")]
    #[allow(clippy::cast_possible_wrap)]
    pub fn now() -> Self {
        let pre = read_timestamp_counter_begin();
        let system_time = RealTime.get_time();
        let post = read_timestamp_counter_end();
        let tsc = pre.midpoint(post);
        Self {
            system_time,
            tsc: TscCount::new(tsc as i64),
        }
    }
}
