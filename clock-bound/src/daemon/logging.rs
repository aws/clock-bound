//! Logging infrastructure for ClockBound.
//!
//! This module provides:
//! - Target constants for routing tracing events to specific log files
//! - A custom [`StructuredLogLayer`] for writing structured JSON log files
//! - Shared serialization types used across log modules
//! - Typed log entry modules (e.g., [`synchronization`]) for each log group
//! - The tracing [`subscriber`] setup that wires everything together
//! - The [`ffevents`] module for typed feed-forward event emission

use serde::Serialize;

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::SourceInfo;
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

/// Serializable description of a clock source for log output.
#[derive(Serialize)]
pub(crate) struct Source {
    #[serde(rename = "type")]
    pub source_type: &'static str,
    pub identifier: String,
}

impl From<SourceInfo> for Source {
    fn from(info: SourceInfo) -> Self {
        match info {
            SourceInfo::AmazonTimeSync(addr, _) | SourceInfo::NtpSource(addr, _) => Self {
                source_type: "ntp",
                identifier: addr.to_string(),
            },
            // TODO: fixme! thread actual device path through `SourceInfo`
            SourceInfo::Phc => Self {
                source_type: "phc",
                identifier: "/dev/ptp0".to_string(),
            },
        }
    }
}

/// Serializable feed-forward clock state for log output.
#[derive(Serialize)]
pub(crate) struct FFClock {
    pub time_sync: String,
    pub clock_error_bound_ns: i64,
    pub status: ClockStatusName,
}

impl FFClock {
    /// Build from clock parameters and current status.
    pub fn from_params(params: &ClockParameters, clock_status: ClockStatus) -> Self {
        let time_sync = chrono::DateTime::from_timestamp_nanos(params.time.as_nanos())
            .format("%Y-%m-%dT%H:%M:%S%.6fZ")
            .to_string();
        Self {
            time_sync,
            clock_error_bound_ns: params.clock_error_bound.as_nanos(),
            status: clock_status.into(),
        }
    }
}
