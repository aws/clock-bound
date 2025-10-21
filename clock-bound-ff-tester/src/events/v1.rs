//! Version 1 of data serialized by `ff-tester`

mod ntp;
use std::collections::HashMap;

use crate::time::TscCount;
pub use ntp::{ClientSystemTimes as NtpClientSystemTimes, Ntp};

mod phc;
pub use phc::{ClientSystemTimes as PhcClientSystemTimes, Phc};
use serde::{Deserialize, Serialize};

mod vmclock;
pub use vmclock::VMClock;

mod oscillator;
pub use oscillator::{Oscillator, OscillatorOffsets};

/// A high level scenario
///
/// This struct holds all time events that occurred (like NTP packet exchanges), oscillator data, as well as applicable metadata
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    /// All time synchronization events that occurred in this scenario
    pub events: Vec<Event>,
    /// The oscillator used in this scenario
    ///
    /// Likely None if this is a real capture. But maybe one day...
    pub oscillator: Option<Oscillator>,
    /// Metadata about the Scenario
    ///
    /// Simple key-value store
    pub metadata: HashMap<String, String>,
}

/// A representation of a time synchronization event.
///
/// An event is characterized of when it was **received** by the local system, hence the tsc timestamp.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// The event information, changes based on the event kind
    pub variants: EventKind,
    /// The local TSC timestamp of when the event was sent
    ///
    /// Clarifications on it's meaning for different event types
    /// - NTP packet: The tsc timestamp when the NTP request was sent on the local system
    /// - PHC read: the tsc timestamp read **before** getting the PHC time reading
    #[serde(alias = "client_raw_send_time")]
    pub client_tsc_pre_time: TscCount,
    /// The local TSC timestamp of when the event was received
    ///
    /// Clarifications on it's meaning for different event types
    /// - NTP packet: The tsc timestamp when the NTP reply was received on the local system
    /// - PHC read: the tsc timestamp read **after** getting the PHC time reading
    /// - PPS pulse: The tsc timestamp when the pulse was received on the local system
    #[serde(alias = "client_raw_recv_time")]
    pub client_tsc_post_time: TscCount,
}

/// Different possible event types and their associated data
///
/// Used to represent the different mechanisms to synchronize time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum EventKind {
    /// NTP request-response
    Ntp(Ntp),
    /// PHC TSC -> PHC -> TSC event
    Phc(Phc),
    /// VMClock read
    VMClock(VMClock),
}

impl EventKind {
    pub fn ntp(&self) -> Option<&Ntp> {
        if let EventKind::Ntp(ntp) = self {
            Some(ntp)
        } else {
            None
        }
    }
    pub fn phc(&self) -> Option<&Phc> {
        if let EventKind::Phc(phc) = self {
            Some(phc)
        } else {
            None
        }
    }
}
