//! Clock synchronization events
//!
//! These are the in-memory representations of NTP and PHC reads.
mod ntp;
pub use ntp::{Ntp, NtpData, Stratum, TryFromU8Error, ValidStratumLevel};

/// A time synchronization event handled by ClockBound
pub enum Event {
    /// NTP Event
    Ntp(Ntp),
}
