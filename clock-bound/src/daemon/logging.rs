//! Logging infrastructure for ClockBound.
//!
//! This module provides:
//! - Target constants for routing tracing events to specific log files
//! - A custom [`StructuredLogLayer`] for writing structured JSON log files
//! - Shared serialization types used across log modules
//! - Typed log entry modules (e.g., [`synchronization`]) for each log group
//! - The tracing [`subscriber`] setup that wires everything together
//! - The [`ffevents`] module for typed feed-forward event emission

use std::net::SocketAddr;

use serde::{Serialize, Serializer};

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::SourceInfo;
use crate::daemon::time::{Duration, Instant};
use crate::shm::ClockStatus;

pub mod ffevents;
pub mod layer;
pub mod subscriber;
pub mod synchronization;

pub use layer::{LogHandle, StructuredLogLayer};

/// Handles to all structured log files, enabling SIGHUP-triggered reopening
/// for lossless log rotation.
///
/// Each field is `Option` to support future configuration where individual
/// log files may be disabled.
///
/// TODO: add other log streams.
pub struct LogHandles {
    ffevents: Option<LogHandle>,
    synchronization: Option<LogHandle>,
}

impl LogHandles {
    /// Reopen all active log files.
    ///
    /// Call this on SIGHUP to support logrotate without `copytruncate`.
    pub fn reopen_all(&self) {
        if let Some(h) = &self.ffevents {
            h.reopen();
        }
        if let Some(h) = &self.synchronization {
            h.reopen();
        }
    }
}

/// Tracing target for the clock metrics log.
pub const CLOCK_METRICS_TARGET: &str = "clock_bound::clock_metrics";

/// Tracing target for the algorithm analysis log (see
/// [`crate::daemon::algo_analysis`]). Only emitted with the `algorithm` feature.
pub const ALGO_ANALYSIS_TARGET: &str = "clock_bound::algo_analysis";

/// All tracing targets that carry high-frequency structured records meant for
/// dedicated log files. When defining a new structured target, add it here so
/// every human-readable output automatically keeps it off.
pub const STRUCTURED_TARGETS: &[&str] =
    &[ffevents::TARGET, CLOCK_METRICS_TARGET, ALGO_ANALYSIS_TARGET];

/// Returns true if a tracing target belongs to one of the structured log streams
/// listed in [`STRUCTURED_TARGETS`].
///
/// Use this to filter structured records out of human-readable layers, e.g.:
/// `filter_fn(|md| !is_structured_target(md.target()))`
pub fn is_structured_target(target: &str) -> bool {
    STRUCTURED_TARGETS.iter().any(|t| target.starts_with(t))
}

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
