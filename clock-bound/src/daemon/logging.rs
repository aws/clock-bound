//! Logging infrastructure for ClockBound.
//!
//! This module provides:
//! - Target constants for routing tracing events to specific log files
//! - A custom [`StructuredLogLayer`] for writing structured JSON log files
//! - Shared serialization types used across log modules
//! - Typed log entry modules (e.g., [`synchronization`]) for each log group
//! - The tracing [`subscriber`] setup that wires everything together

use std::net::SocketAddr;

use serde::{Serialize, Serializer};

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::SourceInfo;
use crate::daemon::time::{Duration, Instant};
use crate::shm::ClockStatus;

pub mod layer;
pub mod subscriber;
pub mod synchronization;

/// Tracing target for the primer reproducibility log.
pub const PRIMER_TARGET: &str = "clock_bound::primer";

/// Tracing target for the clock metrics log.
pub const CLOCK_METRICS_TARGET: &str = "clock_bound::clock_metrics";

/// Log-specific representation of a clock source for serialization.
///
/// Strips stratum information from [`SourceInfo`], keeping only the source
/// identity needed for log output.
pub(crate) enum SourceName {
    None,
    AmazonTimeSync,
    Ntp(SocketAddr),
    Phc,
}

impl From<Option<SourceInfo>> for SourceName {
    fn from(source: Option<SourceInfo>) -> Self {
        match source {
            Option::None => Self::None,
            Some(SourceInfo::AmazonTimeSync(_, _)) => Self::AmazonTimeSync,
            Some(SourceInfo::NtpSource(addr, _)) => Self::Ntp(addr),
            Some(SourceInfo::Phc) => Self::Phc,
        }
    }
}

impl Serialize for SourceName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::None => serializer.serialize_str("null"),
            Self::AmazonTimeSync => serializer.serialize_str("AmazonTimeSync"),
            Self::Ntp(addr) => serializer.serialize_str(&format!("NTP ({addr})")),
            Self::Phc => serializer.serialize_str("PHC"),
        }
    }
}

/// Log-specific representation of clock status for serialization.
///
/// Avoids deriving `Serialize` on the actual [`ClockStatus`] enum.
#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ClockStatusName {
    Unknown,
    Synchronized,
    FreeRunning,
    Disrupted,
}

impl From<ClockStatus> for ClockStatusName {
    fn from(status: ClockStatus) -> Self {
        match status {
            ClockStatus::Unknown => Self::Unknown,
            ClockStatus::Synchronized => Self::Synchronized,
            ClockStatus::FreeRunning => Self::FreeRunning,
            ClockStatus::Disrupted => Self::Disrupted,
        }
    }
}

/// Clock parameters subset for log entries: timestamp and clock error bound.
pub(crate) struct ClockInfo {
    pub timestamp: Instant,
    pub clock_error_bound: Duration,
}

impl Serialize for ClockInfo {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut s = serializer.serialize_struct("ClockInfo", 2)?;
        let ts = chrono::DateTime::from_timestamp_nanos(self.timestamp.as_nanos())
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string();
        s.serialize_field("timestamp", &ts)?;
        s.serialize_field("clock_error_bound_ns", &self.clock_error_bound.as_nanos())?;
        s.end()
    }
}

impl From<&ClockParameters> for ClockInfo {
    fn from(params: &ClockParameters) -> Self {
        Self {
            timestamp: params.time,
            clock_error_bound: params.clock_error_bound,
        }
    }
}
