//! Clock synchronization events
//!
//! These are the in-memory representations of NTP and PHC reads.
mod ntp;
pub use ntp::{Ntp, NtpData, Stratum, TryFromU8Error, ValidStratumLevel};

mod phc;
pub use phc::{Phc, PhcData};

use crate::daemon::io::tsc::{read_timestamp_counter_begin, read_timestamp_counter_end};
use crate::daemon::time::{Clock, TscCount, TscDiff, clocks::RealTime};

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
    pub fn now() -> Self {
        let pre = read_timestamp_counter_begin();
        let system_time = RealTime.get_time();
        let post = read_timestamp_counter_end();
        let tsc = pre.midpoint(post);
        Self {
            system_time,
            tsc: TscCount::new(tsc.into()),
        }
    }
}
