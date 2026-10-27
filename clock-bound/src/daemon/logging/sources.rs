//! Sources log entry types and emit helpers.
//!
//! The sources log (`sources.log`) emits periodic snapshots describing every time source known
//! to the daemon, selected or not. It records each source's identity, selection history, health,
//! and sample statistics, and backs the `cbctl sources` command.
//!
//! ## Usage
//!
//! ```ignore
//! use crate::daemon::logging::sources::log_sources_snapshot;
//!
//! log_sources_snapshot(&sources_snapshot_data);
//! ```

use serde::Serialize;

use super::{ClockStatusName, LogEvent, SourceType, StructuredLog, emit_event};
use crate::daemon::clock_sync_algorithm::SourceInfo;
use crate::daemon::clock_sync_algorithm::ff::{SkmSummary, StatSummary};
use crate::daemon::time::Duration;

/// How often the daemon emits a snapshot to `sources.log`.
pub const SNAPSHOT_PERIOD_SECS: u64 = 60;

/// The window over which per-source sample statistics are computed and reported.
///
/// Clamped to the sample buffer's own window if larger (see
/// [`skm_summary_of`](crate::daemon::clock_sync_algorithm::ff)).
pub const STATS_WINDOW: Duration = Duration::from_minutes(10);

/// Caller-facing snapshot data: the per-source data for one snapshot.
#[derive(Debug, Clone, Default)]
pub struct SourceSnapshotData {
    pub sources: Vec<SourceData>,
}

/// Caller-facing data for a single source.
#[derive(Debug, Clone)]
pub struct SourceData {
    pub source_info: SourceInfo,
    pub poll_interval: Duration,
    pub skm: SkmSummary,
}

/// Body of a `sources_snapshot` event.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct SourcesSnapshot {
    sources: Vec<SourceRow>,
}

impl LogEvent for SourcesSnapshot {
    const STREAM: StructuredLog = StructuredLog::Sources;
    const EVENT: &'static str = "sources_snapshot";
}

impl From<&SourceSnapshotData> for SourcesSnapshot {
    fn from(data: &SourceSnapshotData) -> Self {
        Self {
            sources: data.sources.iter().map(SourceRow::from).collect(),
        }
    }
}

/// One source's full state within a snapshot.
///
/// Every field is always present; absent data is `null` within a field, never a
/// missing key, so all rows share one schema regardless of warm-up.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct SourceRow {
    #[serde(rename = "type")]
    source_type: SourceType,
    /// Address (`ip:port`) or device path (`/dev/ptp0`).
    identifier: String,
    /// Configured poll interval, in seconds.
    poll_interval_sec: f64,
    health: Health,
    stats: Stats,
}

impl From<&SourceData> for SourceRow {
    fn from(data: &SourceData) -> Self {
        // TODO: populate health and stats as they are wired in.
        Self {
            source_type: SourceType::from(&data.source_info),
            identifier: data.source_info.identifier(),
            poll_interval_sec: data.poll_interval.as_seconds_f64(),
            health: Health {
                selected_at: None,
                deselected_at: None,
                status: ClockStatusName::Unknown,
            },
            stats: Stats::from(&data.skm),
        }
    }
}

/// A source's selection history and current responsiveness.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Health {
    /// When the source began its current (or most recent) selection tenure, or
    /// `None` if never selected.
    selected_at: Option<String>,
    /// When the source's last selection tenure ended, or `None` if currently
    /// selected or never selected.
    deselected_at: Option<String>,
    status: ClockStatusName,
}

/// A min/median/max summary for log output, in nanoseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
struct StatSummaryNs {
    min: i64,
    median: i64,
    max: i64,
}

impl From<StatSummary<Duration>> for StatSummaryNs {
    fn from(summary: StatSummary<Duration>) -> Self {
        Self {
            min: summary.min.as_nanos(),
            median: summary.median.as_nanos(),
            max: summary.max.as_nanos(),
        }
    }
}

/// Aggregates computed over a source's sample buffer at snapshot time.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Stats {
    requests: u64,
    responses: u64,
    /// Measurement latency distribution over the window, or `None` if empty or
    /// no period is available to scale it.
    latency_ns: Option<StatSummaryNs>,
    /// Source-reported error bound distribution over the window, or `None` if
    /// empty.
    source_clock_error_bound_ns: Option<StatSummaryNs>,
    /// Duration the stats window spans, in seconds.
    window_duration_sec: u64,
}

impl From<&SkmSummary> for Stats {
    fn from(skm: &SkmSummary) -> Self {
        // TODO: populate request/response counters as they are wired in.
        Self {
            requests: 0,
            responses: 0,
            latency_ns: skm.latency.map(StatSummaryNs::from),
            source_clock_error_bound_ns: skm.source_clock_error_bound.map(StatSummaryNs::from),
            window_duration_sec: skm.window.as_seconds().unsigned_abs(),
        }
    }
}

/// Log a sources snapshot to the sources log.
pub fn log_sources_snapshot(data: &SourceSnapshotData) {
    emit_event(&SourcesSnapshot::from(data));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::clock_sync_algorithm::SourceInfo;
    use crate::daemon::event::Stratum;
    use crate::daemon::logging::{
        StructuredLog,
        test_support::{assert_matches_expected, with_layer},
    };
    use crate::daemon::time::Duration;
    use indoc::indoc;

    const TEST_VERSION: &str = "0.1.0-test";

    #[test]
    fn log_empty_sources_snapshot_schema() {
        let (output, _tmp) = with_layer(StructuredLog::Sources, TEST_VERSION, || {
            log_sources_snapshot(&SourceSnapshotData::default());
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            indoc! {r#"
            {
                "timestamp": "",
                "event": "sources_snapshot",
                "sources_snapshot": {
                    "sources": []
                },
                "version": "0.1.0-test"
            }
            "#},
        );
    }

    #[test]
    fn log_sources_snapshot_identity_schema() {
        let data = SourceSnapshotData {
            sources: vec![
                SourceData {
                    source_info: SourceInfo::NtpSource(
                        "169.254.169.123:123".parse().unwrap(),
                        Stratum::Unspecified,
                    ),
                    poll_interval: Duration::from_secs(16),
                    skm: SkmSummary::default(),
                },
                SourceData {
                    source_info: SourceInfo::Phc("/dev/ptp0".into()),
                    poll_interval: Duration::from_millis(500),
                    skm: SkmSummary::default(),
                },
            ],
        };

        let (output, _tmp) = with_layer(StructuredLog::Sources, TEST_VERSION, || {
            log_sources_snapshot(&data);
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            indoc! {r#"
            {
                "timestamp": "",
                "event": "sources_snapshot",
                "sources_snapshot": {
                    "sources": [
                        {
                            "type": "ntp",
                            "identifier": "169.254.169.123:123",
                            "poll_interval_sec": 16.0,
                            "health": {
                                "selected_at": null,
                                "deselected_at": null,
                                "status": "UNKNOWN"
                            },
                            "stats": {
                                "requests": 0,
                                "responses": 0,
                                "latency_ns": null,
                                "source_clock_error_bound_ns": null,
                                "window_duration_sec": 0
                            }
                        },
                        {
                            "type": "phc",
                            "identifier": "/dev/ptp0",
                            "poll_interval_sec": 0.5,
                            "health": {
                                "selected_at": null,
                                "deselected_at": null,
                                "status": "UNKNOWN"
                            },
                            "stats": {
                                "requests": 0,
                                "responses": 0,
                                "latency_ns": null,
                                "source_clock_error_bound_ns": null,
                                "window_duration_sec": 0
                            }
                        }
                    ]
                },
                "version": "0.1.0-test"
            }
            "#},
        );
    }

    #[test]
    fn log_sources_snapshot_stats_schema() {
        let data = SourceSnapshotData {
            sources: vec![SourceData {
                source_info: SourceInfo::NtpSource(
                    "169.254.169.123:123".parse().unwrap(),
                    Stratum::Unspecified,
                ),
                poll_interval: Duration::from_secs(16),
                skm: SkmSummary {
                    window: Duration::from_secs(1024),
                    latency: Some(StatSummary {
                        min: Duration::from_nanos(128_400),
                        median: Duration::from_nanos(141_200),
                        max: Duration::from_nanos(155_000),
                    }),
                    source_clock_error_bound: Some(StatSummary {
                        min: Duration::from_nanos(27_800),
                        median: Duration::from_nanos(30_100),
                        max: Duration::from_nanos(31_200),
                    }),
                },
            }],
        };

        let (output, _tmp) = with_layer(StructuredLog::Sources, TEST_VERSION, || {
            log_sources_snapshot(&data);
        });

        assert_matches_expected(
            output.lines().next().unwrap(),
            indoc! {r#"
            {
                "timestamp": "",
                "event": "sources_snapshot",
                "sources_snapshot": {
                    "sources": [
                        {
                            "type": "ntp",
                            "identifier": "169.254.169.123:123",
                            "poll_interval_sec": 16.0,
                            "health": {
                                "selected_at": null,
                                "deselected_at": null,
                                "status": "UNKNOWN"
                            },
                            "stats": {
                                "requests": 0,
                                "responses": 0,
                                "latency_ns": { "min": 128400, "median": 141200, "max": 155000 },
                                "source_clock_error_bound_ns": { "min": 27800, "median": 30100, "max": 31200 },
                                "window_duration_sec": 1024
                            }
                        }
                    ]
                },
                "version": "0.1.0-test"
            }
            "#},
        );
    }

    #[test]
    fn emit_is_noop_without_tracing_layer() {
        use tempfile::TempDir;

        let tmp_dir = TempDir::new().unwrap();
        let log_path = tmp_dir.path().join(StructuredLog::Sources.file_name());

        log_sources_snapshot(&SourceSnapshotData::default());

        assert!(
            !log_path.exists(),
            "log file should not be created when no tracing layer is registered"
        );
    }
}
