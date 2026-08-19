//! Logging infrastructure for ClockBound.
//!
//! This module provides:
//! - The [`StructuredLog`] enum, binding each structured stream to its tracing target and on-disk log file
//! - A custom [`StructuredLogLayer`](layer::StructuredLogLayer) for writing structured JSON log files
//! - Shared serialization types used across log modules
//! - Typed log entry modules (e.g., [`synchronization`]) for each log group
//! - The tracing [`subscriber`] setup that wires everything together
//! - The [`ffevents`] module for typed feed-forward event emission

use serde::Serialize;

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::{SourceInfo, SyncParameters};
use crate::shm::ClockStatus;

pub mod ffevents;
pub mod layer;
pub mod subscriber;
pub mod synchronization;

use layer::LogHandle;

/// Handles to all structured log files, enabling SIGHUP-triggered reopening
/// for lossless log rotation.
///
/// Holds one [`LogHandle`] per active structured stream. Individual streams can
/// be disabled simply by not pushing a handle for them during subscriber setup;
/// [`reopen_all`](LogHandles::reopen_all) only reopens the handles present.
///
/// TODO: add other log streams.
pub struct LogHandles {
    handles: Vec<LogHandle>,
}

impl LogHandles {
    /// Reopen all active log files.
    ///
    /// Call this on SIGHUP to support logrotate without `copytruncate`.
    pub fn reopen_all(&self) {
        for handle in &self.handles {
            handle.reopen();
        }
    }
}

/// Tracing target for the clock metrics log.
pub(crate) const CLOCK_METRICS_TARGET: &str = "clock_bound::clock_metrics";

/// Identifies a structured log stream, binding its tracing target and on-disk
/// file name together at a single definition site.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StructuredLog {
    /// Feed-forward events log (see [`ffevents`]).
    FFEvents,
    /// Synchronization log (see [`synchronization`]).
    Synchronization,
    /// Algorithm analysis log (see [`crate::daemon::algo_analysis`]). Only
    /// emitted with the `algorithm` feature.
    AlgoAnalysis,
}

impl StructuredLog {
    /// Every [`StructuredLog`] variant.
    const ALL: &'static [StructuredLog] = &[
        StructuredLog::FFEvents,
        StructuredLog::Synchronization,
        StructuredLog::AlgoAnalysis,
    ];

    /// Tracing target used to route this stream's records to its layer.
    pub const fn target(self) -> &'static str {
        match self {
            Self::FFEvents => "clock_bound::ffevents",
            Self::Synchronization => "clock_bound::synchronization",
            Self::AlgoAnalysis => "clock_bound::algo_analysis",
        }
    }

    /// On-disk file name for this stream's log file.
    const fn file_name(self) -> &'static str {
        match self {
            Self::FFEvents => "ffevents.log",
            Self::Synchronization => "synchronization.log",
            Self::AlgoAnalysis => "algo_analysis.log",
        }
    }
}

/// Returns true if a tracing target belongs to one of the structured log streams.
pub fn is_structured_target(target: &str) -> bool {
    StructuredLog::ALL
        .iter()
        .map(|stream| stream.target())
        // CLOCK_METRICS_TARGET has no StructuredLog variant because that stream
        // is slated for deletion; until then it must still be filtered out of
        // the human-readable layers, so append it as a one-off here.
        .chain(std::iter::once(CLOCK_METRICS_TARGET))
        .any(|t| target.starts_with(t))
}

/// A structured log event routed to one of the dedicated log files.
pub(crate) trait LogEvent: Serialize {
    /// The structured log stream this event is routed to. Binds the event type
    /// to its destination at compile time.
    const STREAM: StructuredLog;

    /// Discriminator string used for both the `"event"` value and the nested content key.
    const EVENT: &'static str;
}

/// Emit a typed [`LogEvent`] to its bound structured log stream ([`LogEvent::STREAM`]).
///
/// Requires a `StructuredLogLayer` registered for the event's stream; otherwise
/// the record is silently dropped.
pub(crate) fn emit_event<E: LogEvent>(event: &E) {
    emit_event_inner(event, None);
}

/// Emit a typed [`LogEvent`] with an accompanying sequence number.
///
/// Identical to [`emit_event`], but inserts a top-level `"seq"` field (between
/// `"event"` and the body) carrying `seq`. Used by the ffevents log to number
/// every `init`/`feed`/`disruption` record monotonically, starting at zero.
pub(crate) fn emit_event_with_seq<E: LogEvent>(event: &E, seq: u64) {
    emit_event_inner(event, Some(seq));
}

/// Shared envelope builder for [`emit_event`] and [`emit_event_seq`].
fn emit_event_inner<E: LogEvent>(event: &E, seq: Option<u64>) {
    let body = serde_json::to_value(event).unwrap();

    // Envelope ordering (event, then optional seq, then body) is significant:
    // serde_json has `preserve_order` enabled, so map insertion order is the
    // on-disk order.
    let mut envelope = serde_json::Map::new();
    envelope.insert("event".to_string(), serde_json::Value::from(E::EVENT));
    if let Some(seq) = seq {
        envelope.insert("seq".to_string(), serde_json::Value::from(seq));
    }
    envelope.insert(E::EVENT.to_string(), body);

    let Ok(json) = serde_json::to_string(&envelope) else {
        return;
    };

    emit_entry(E::STREAM, &json);
}

/// Forward a pre-serialized `entry` string to the [`StructuredLogLayer`]
/// registered for `stream`.
///
/// The `entry` field MUST be recorded as a `&str`.
fn emit_entry(stream: StructuredLog, entry: &str) {
    match stream {
        StructuredLog::FFEvents => {
            tracing::info!(target: StructuredLog::FFEvents.target(), entry = entry);
        }
        StructuredLog::Synchronization => {
            tracing::info!(target: StructuredLog::Synchronization.target(), entry = entry);
        }
        StructuredLog::AlgoAnalysis => {
            tracing::info!(target: StructuredLog::AlgoAnalysis.target(), entry = entry);
        }
    }
}

/// Canonical timestamp format used across all structured log fields
/// (`timestamp`, `time_sync`, `selected_at`, etc.): RFC 3339 with microsecond
/// precision and a literal `Z` UTC suffix.
const LOG_TIMESTAMP_FORMAT: &str = "%Y-%m-%dT%H:%M:%S%.6fZ";

/// Format a UTC instant for structured log output.
pub(crate) fn format_log_timestamp(ts: chrono::DateTime<chrono::Utc>) -> String {
    ts.format(LOG_TIMESTAMP_FORMAT).to_string()
}

/// Log-specific representation of clock status for serialization.
///
/// Avoids deriving `Serialize` on the actual [`ClockStatus`] enum.
#[derive(Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ClockStatusName {
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
struct Source {
    #[serde(rename = "type")]
    #[expect(clippy::struct_field_names, reason = "intentional")]
    source_type: &'static str,
    identifier: String,
    /// When the selected source began its current selection tenure.
    selected_at: String,
    /// The source's clock error bound (in nanoseconds) at the start of its
    /// current selection tenure.
    clock_error_bound_ns: i64,
}

impl From<&SyncParameters> for Source {
    fn from(params: &SyncParameters) -> Self {
        let selected_at = format_log_timestamp(chrono::DateTime::from_timestamp_nanos(
            params.selected_at.as_nanos(),
        ));
        let clock_error_bound_ns = params.selected_at_clock_error_bound.as_nanos();
        match &params.source_info {
            SourceInfo::AmazonTimeSync(addr, _) | SourceInfo::NtpSource(addr, _) => Self {
                source_type: "ntp",
                identifier: addr.to_string(),
                selected_at,
                clock_error_bound_ns,
            },
            SourceInfo::Phc(device_path) => Self {
                source_type: "phc",
                identifier: device_path.to_string(),
                selected_at,
                clock_error_bound_ns,
            },
        }
    }
}

/// Serializable feed-forward clock state for log output.
#[derive(Serialize)]
struct FFClock {
    time_sync: String,
    clock_error_bound_ns: i64,
    status: ClockStatusName,
}

impl FFClock {
    /// Build from clock parameters and current status.
    fn from_params(params: &ClockParameters, clock_status: ClockStatus) -> Self {
        let time_sync = format_log_timestamp(chrono::DateTime::from_timestamp_nanos(
            params.time.as_nanos(),
        ));
        Self {
            time_sync,
            clock_error_bound_ns: params.clock_error_bound.as_nanos(),
            status: clock_status.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StructuredLog;

    #[test]
    fn all_lists_every_variant() {
        for variant in [
            StructuredLog::FFEvents,
            StructuredLog::Synchronization,
            StructuredLog::AlgoAnalysis,
        ] {
            // Exhaustiveness guard: adding a variant breaks compilation here.
            match variant {
                StructuredLog::FFEvents
                | StructuredLog::Synchronization
                | StructuredLog::AlgoAnalysis => {}
            }
            assert!(
                StructuredLog::ALL.contains(&variant),
                "{variant:?} missing from StructuredLog::ALL"
            );
        }

        assert_eq!(StructuredLog::ALL.len(), 3);
    }
}

/// Shared test scaffolding for structured-log modules.
///
/// The per-module test suites (e.g. [`ffevents`], [`synchronization`]) all need
/// to (a) stand up a [`StructuredLogLayer`] writing to a tempdir and (b) compare
/// a parsed output line against expected JSON with the dynamic `timestamp`
/// copied over.
#[cfg(test)]
pub(crate) mod test_support {
    use std::fs::File;
    use std::io::Read;

    use serde_json::Value;
    use tempfile::TempDir;
    use tracing_subscriber::layer::SubscriberExt;

    use super::StructuredLog;
    use super::layer::StructuredLogLayer;

    /// Set up a [`StructuredLogLayer`] for `stream` writing to a fresh tempdir,
    /// run `f` with the subscriber active, and return the file contents. The
    /// [`TempDir`] is returned to keep it alive for the caller's scope.
    pub(crate) fn with_layer(
        stream: StructuredLog,
        version: &'static str,
        f: impl FnOnce(),
    ) -> (String, TempDir) {
        let tmp_dir = TempDir::new().unwrap();
        let (layer, _handle) = StructuredLogLayer::for_stream(tmp_dir.path(), stream, version);

        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, f);

        let mut contents = String::new();
        File::open(tmp_dir.path().join(stream.file_name()))
            .unwrap()
            .read_to_string(&mut contents)
            .unwrap();
        (contents, tmp_dir)
    }

    /// Parse `actual_line` and compare it against `expected_json`, copying the
    /// dynamic `timestamp` from actual into expected before asserting equality.
    pub(crate) fn assert_matches_expected(actual_line: &str, expected_json: &str) {
        let actual: Value =
            serde_json::from_str(actual_line).expect("actual output is not valid JSON");
        let mut expected: Value =
            serde_json::from_str(expected_json).expect("expected JSON is not valid JSON");

        // Timestamp is dynamic — copy from actual so comparison passes.
        expected["timestamp"] = actual["timestamp"].clone();

        assert_eq!(actual, expected);
    }
}
