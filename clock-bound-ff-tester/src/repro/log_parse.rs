//! Functions and structs related to parsing log files: the daemon's `ffevents.log`, structured
//! NDJSON (ClockBound 3.x).

use anyhow::Context;

use clock_bound::daemon::{
    clock_parameters::ClockParameters,
    logging::ffevents::types::{ClockParametersLog, RoutableEventLog},
};

use super::ReproEvent;

use std::{io::BufRead, path::Path};

// Only used by the test-only eager wrapper
#[cfg(test)]
use std::io::{BufReader, Read};

/// Open a log file and return a streaming [`ReproEventReader`] over its events.
///
/// The logfile may be gzip compressed (detected from content, not file extension).
///
/// The input must be a native daemon log: lines carrying delivery prefixes (e.g.
/// pulled via `aws logs tail`) are rejected — fix such logs once with the
/// `fix-log-order` command before replaying.
///
/// # Errors
/// - Returns an error if unable to open or read the file.
pub fn repro_events_from_log_file(
    file_path: impl AsRef<Path>,
) -> anyhow::Result<ReproEventReader<Box<dyn BufRead>>> {
    Ok(ReproEventReader::new(crate::logs::open_log(file_path)?))
}

/// Read a whole in-memory log and return its [`ReproEvent`] items, in file order; an error when
/// it holds none.
///
/// A test utility: the eager form of [`ReproEventReader`] for fixtures. Production paths stream
/// the reader.
///
/// # Errors
/// Returns errors if the logfile is corrupted or contains no events.
#[cfg(test)]
pub(crate) fn repro_events_from_reader(input: impl Read) -> anyhow::Result<Vec<ReproEvent>> {
    let events: Vec<ReproEvent> =
        ReproEventReader::new(BufReader::new(input)).collect::<anyhow::Result<_>>()?;
    anyhow::ensure!(!events.is_empty(), "No events found in log file");
    Ok(events)
}

/// Streaming reader converting a log into a sequence of [`ReproEvent`] items.
///
/// Implements [`Iterator`] with `Item = anyhow::Result<ReproEvent>`, reading and
/// parsing one line at a time: memory usage is bounded by the longest line, not the
/// file size. Events are yielded in file order, which is the order the daemon
/// experienced them; timestamps are *not* sorted. Ordering anomalies are classified
/// and reported (see [`crate::logs::order`]): a delivery scramble warns and points
/// at the `fix-log-order` command, while clock steps and async I/O reordering are
/// the daemon's own dynamics, reported at info level with their magnitude.
///
/// The input must be native daemon NDJSON: every line must start with `{`. Lines
/// carrying delivery prefixes (e.g. `aws logs tail`) yield an error directing to
/// `fix-log-order`. Unparseable JSON lines are skipped with a warning.
///
/// The input must be buffered ([`BufRead`]); file access and gzip handling are
/// provided by [`repro_events_from_log_file`].
pub struct ReproEventReader<R: BufRead> {
    lines: std::io::Lines<R>,
    /// Number of lines consumed from the input so far
    lines_read: usize,
    /// Ordering anomaly classification across the stream
    order_tracker: crate::logs::order::OrderTracker,
}

impl<R: BufRead> ReproEventReader<R> {
    /// Create a streaming reader over `input`.
    pub fn new(input: R) -> Self {
        Self {
            lines: input.lines(),
            lines_read: 0,
            order_tracker: crate::logs::order::OrderTracker::new(),
        }
    }

    /// Number of lines consumed from the input so far.
    pub fn lines_read(&self) -> usize {
        self.lines_read
    }

    /// Summary of ordering anomalies observed so far (see [`crate::logs::order`]).
    pub fn order_summary(&self) -> &crate::logs::order::OrderSummary {
        self.order_tracker.summary()
    }

    /// Parse one line into an event.
    ///
    /// Returns `Ok(None)` for lines to skip (already warned about), `Err` for
    /// structural failures that the caller should see.
    fn parse_line(line: &str, idx: usize) -> anyhow::Result<Option<ReproEvent>> {
        // Strict input contract: native daemon NDJSON only. A prefixed line means
        // the log came through a delivery pipeline and must be fixed first.
        if !line.starts_with('{') {
            anyhow::bail!(
                "Line {idx} does not start with a JSON object. If this log was pulled \
                 from a delivery pipeline (e.g. `aws logs tail`), run `fix-log-order` \
                 on it first: {line}"
            );
        }

        match parse_ffevents_line(line, idx) {
            // The timestamp is carried per-line for ordering diagnostics but is
            // not part of the yielded event
            Ok((_timestamp, event)) => Ok(Some(event)),
            Err(e) => {
                tracing::warn!("Could not parse line {idx} from log file: {e}");
                Ok(None)
            }
        }
    }
}

impl<R: BufRead> Iterator for ReproEventReader<R> {
    type Item = anyhow::Result<ReproEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let Ok(line) = self.lines.next()? else {
                tracing::warn!("Could not read line from log file");
                continue;
            };

            let idx = self.lines_read;
            self.lines_read += 1;

            // Ordering diagnostics on the raw line: events are yielded in file
            // order regardless; anomalies are classified and reported.
            let signals = crate::logs::order::OrderingSignals::extract(&line);
            if let Some(anomaly) = self.order_tracker.observe(idx, &signals) {
                use crate::logs::order::OrderAnomalyKind;
                match anomaly.kind() {
                    OrderAnomalyKind::DeliveryScramble => {
                        tracing::warn!(
                            "{anomaly}: input appears delivery-scrambled, consider \
                             running `fix-log-order` on it"
                        );
                    }
                    OrderAnomalyKind::ClockStep | OrderAnomalyKind::AsyncReorder => {
                        tracing::info!("{anomaly}");
                    }
                }
            }

            match Self::parse_line(&line, idx) {
                Ok(Some(event)) => return Some(Ok(event)),
                // Skipped line (already warned about): fall through to the next
                Ok(None) => {}
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

// --- ffevents format parsing ---

/// Parse a single ffevents-format JSON line
fn parse_ffevents_line(json_str: &str, idx: usize) -> anyhow::Result<(String, ReproEvent)> {
    let parsed: FfeventsLine = serde_json::from_str(json_str)
        .with_context(|| format!("Line {idx}: failed to parse as ffevents format"))?;

    let repro_event = match parsed.event.as_str() {
        "init" => ReproEvent::Init,
        "disruption" => ReproEvent::Disruption,
        "feed" => {
            let feed = parsed.feed.ok_or_else(|| {
                anyhow::anyhow!("Line {idx}: event is 'feed' but no 'feed' payload found")
            })?;
            ReproEvent::Feed {
                event: feed
                    .input
                    .try_into()
                    .with_context(|| format!("Line {idx}: invalid ffevents feed event"))?,
                output: feed.output.map(ClockParameters::from),
            }
        }
        other => anyhow::bail!("Line {idx}: unknown event type: {other}"),
    };

    Ok((parsed.timestamp, repro_event))
}

/// ffevents.log line structure
#[derive(Debug, Clone, serde::Deserialize)]
struct FfeventsLine {
    timestamp: String,
    #[allow(dead_code)]
    version: Option<String>,
    event: String,
    #[serde(default)]
    feed: Option<FeedPayload>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct FeedPayload {
    input: RoutableEventLog,
    output: Option<ClockParametersLog>,
}

#[cfg(test)]
mod tests {
    use clock_bound::daemon::event::TscRtt;

    use super::*;

    #[test]
    fn parse_ffevents_log() {
        let log = include_str!("test_ffevents.log");
        let mut reader = ReproEventReader::new(log.as_bytes());
        let events: Vec<_> = (&mut reader).collect::<anyhow::Result<_>>().unwrap();

        // 1 init + 5 feeds + 1 disruption + 1 feed = 8 lines total, every one consumed
        assert_eq!(events.len(), 8);
        assert_eq!(reader.lines_read(), log.lines().count());

        // First event is init
        assert!(matches!(events[0], ReproEvent::Init));

        // Count feed events with output (clock parameters)
        let param_count = events
            .iter()
            .filter_map(|event| {
                let ReproEvent::Feed { output, .. } = event else {
                    return None;
                };
                output.as_ref()
            })
            .count();
        assert_eq!(param_count, 2);

        // Verify disruption is present
        let disruption_count = events
            .iter()
            .filter(|e| matches!(e, ReproEvent::Disruption))
            .count();
        assert_eq!(disruption_count, 1);
    }

    #[test]
    fn parse_ffevents_log_events_sorted_by_timestamp() {
        let log = include_str!("test_ffevents.log");
        let events = repro_events_from_reader(log.as_bytes()).unwrap();

        // Extract feed events and verify they're sorted by counter_pre
        let feed_events: Vec<_> = events
            .iter()
            .filter_map(|e| {
                let ReproEvent::Feed { event, .. } = e else {
                    return None;
                };
                Some(event)
            })
            .collect();

        let is_sorted = feed_events.is_sorted_by_key(|e| e.counter_pre());
        assert!(is_sorted);
    }

    #[test]
    fn cloudwatch_prefixed_lines_are_rejected() {
        let log = r#"2026-07-16T14:33:49.000000+00:00 i-abc123 {"timestamp":"2026-07-16T14:33:49.654418Z","version":"3.0.0-alpha.2","event":"feed","feed":{"input":{"type":"ntp","identifier":"169.254.169.123:123","counter_pre":482393264419,"counter_post":482393405301,"data":{"server_recv_time_ns":1784212429654307727,"server_send_time_ns":1784212429654319674,"root_delay_ns":30517578125000,"root_dispersion_ns":15258789063000,"stratum":1}},"output":null}}
"#;
        let error = repro_events_from_reader(log.as_bytes()).unwrap_err();
        assert!(error.to_string().contains("fix-log-order"));
    }

    #[test]
    fn cloudwatch_log_replays_after_fix_log_order() {
        use clap::Parser;

        let log = include_str!("cw_tail_2026_09_28.log");

        // Raw CloudWatch pulls are rejected by the replay path...
        let error = repro_events_from_reader(log.as_bytes()).unwrap_err();
        assert!(error.to_string().contains("fix-log-order"));

        // ... and become replayable after a fix-log-order pass
        let input = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(input.path(), log).unwrap();
        let out_dir = tempfile::TempDir::new().unwrap();
        let output = out_dir.path().join("fixed.log");
        let cli = crate::busybox_bins::fix_log_order::Cli::parse_from([
            "fix-log-order",
            "--logfile",
            input.path().to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
        ]);
        crate::busybox_bins::fix_log_order::main(cli).unwrap();

        let events: Vec<_> = repro_events_from_log_file(&output)
            .unwrap()
            .collect::<anyhow::Result<_>>()
            .unwrap();
        let feed_events: Vec<_> = events
            .iter()
            .filter_map(|e| {
                let ReproEvent::Feed { event, .. } = e else {
                    return None;
                };
                Some(event)
            })
            .collect();

        assert_eq!(feed_events.len(), 13);

        // The fix repaired the delivery scramble: counters are in order
        let is_sorted = feed_events.is_sorted_by_key(|e| e.counter_pre());
        assert!(is_sorted);
    }

    #[test]
    fn plaintext_log_file_parses_same_as_reader() {
        let example_log = include_str!("test_ffevents.log");
        let expected = repro_events_from_reader(example_log.as_bytes()).unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), example_log).unwrap();

        let events: Vec<_> = repro_events_from_log_file(tmp.path())
            .unwrap()
            .collect::<anyhow::Result<_>>()
            .unwrap();
        assert!(!events.is_empty());
        assert_eq!(format!("{events:?}"), format!("{expected:?}"));
    }

    #[test]
    fn gzip_log_file_round_trips() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;

        let example_log = include_str!("test_ffevents.log");
        let expected = repro_events_from_reader(example_log.as_bytes()).unwrap();

        // Compress as two concatenated gzip members, as produced by e.g.
        // `cat a.log.gz b.log.gz`: the full stream must be decoded, not just the
        // first member (which is all `GzDecoder` would return, unlike
        // `MultiGzDecoder`).
        let (first, second) = example_log.as_bytes().split_at(example_log.len() / 2);
        let mut compressed = Vec::new();
        for part in [first, second] {
            let mut encoder = GzEncoder::new(&mut compressed, Compression::default());
            encoder.write_all(part).unwrap();
            encoder.finish().unwrap();
        }

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), &compressed).unwrap();

        let events: Vec<_> = repro_events_from_log_file(tmp.path())
            .unwrap()
            .collect::<anyhow::Result<_>>()
            .unwrap();
        assert!(!events.is_empty());
        assert_eq!(format!("{events:?}"), format!("{expected:?}"));
    }

    #[test]
    fn events_are_yielded_in_file_order_not_sorted() {
        // Swap two feed lines: events must come out in file order, not be
        // reordered by timestamp.
        let log = include_str!("test_ffevents.log");
        let mut lines: Vec<&str> = log.lines().collect();
        lines.swap(1, 2);
        let swapped = lines.join("\n");

        let original = repro_events_from_reader(log.as_bytes()).unwrap();
        let mut expected: Vec<_> = original;
        expected.swap(1, 2);

        let events = repro_events_from_reader(swapped.as_bytes()).unwrap();
        assert_eq!(format!("{events:?}"), format!("{expected:?}"));
    }

    #[test]
    fn empty_input_yields_no_events() {
        let mut reader = ReproEventReader::new("".as_bytes());
        assert!(reader.next().is_none());
        assert_eq!(reader.lines_read(), 0);

        // The eager wrapper keeps the previous "no events" error behavior
        assert!(repro_events_from_reader("".as_bytes()).is_err());
    }
}
