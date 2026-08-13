//! Functions and structs related to parsing log files
//!
//! Supports two log formats:
//! - **ffevents**: The current structured NDJSON format (ClockBound 3.x+)
//! - **primer**: The legacy tracing-subscriber JSON format (ClockBound ≤2.x)
//!
//! Format is auto-detected from the structure of the first parseable line.

use anyhow::Context;
use flate2::read::MultiGzDecoder;

use clock_bound::daemon::{clock_parameters::ClockParameters, receiver_stream::RoutableEvent};

use super::{ReproEvent, log_format_primer};

use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

/// The detected log format for a given log file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LogFormat {
    /// Current ffevents.log format (ClockBound 3.x+)
    Ffevents,
    /// Legacy primer.log format (ClockBound ≤2.x)
    Primer,
}

/// Magic number identifying a gzip compressed file
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Read a logfile and return all inputs and outputs from the `ClockSyncAlgorithm`
///
/// The logfile may be gzip compressed (detected from content, not file extension).
/// Concatenated gzip members (e.g. rotated logs joined with `cat`) are decoded in full.
///
/// # Errors
/// - Returns an error if unable to open the file.
/// - Returns an error if unable to read values from the logfile
pub fn repro_events_from_log_file(file_path: impl AsRef<Path>) -> anyhow::Result<Vec<ReproEvent>> {
    let mut reader = BufReader::new(File::open(&file_path)?);

    // Peek at the first bytes to detect compression: fill_buf() reads at most one BufReader
    // buffer's worth (8 KiB by default, one read syscall) and returns a borrowed slice without
    // consuming it, so the bytes remain for the actual parsing below. An empty or 1-byte file
    // yields a short buffer, which simply fails the match.
    let is_gzip = reader.fill_buf()?.starts_with(&GZIP_MAGIC);
    if is_gzip {
        repro_events_from_reader(MultiGzDecoder::new(reader))
    } else {
        repro_events_from_reader(reader)
    }
}

/// Convert a log file into a sequence of [`ReproEvent`] items.
///
/// Auto-detects whether the file is in ffevents or primer format based on the
/// structure of the first parseable JSON line.
///
/// # Errors
/// Returns errors if the logfile is corrupted or contains no events.
pub fn repro_events_from_reader(input: impl Read) -> anyhow::Result<Vec<ReproEvent>> {
    let reader = BufReader::new(input);
    let mut events = Vec::new();
    let mut detected_format: Option<LogFormat> = None;
    let mut primer_femto_mode: Option<bool> = None;

    for (idx, line) in reader.lines().enumerate() {
        let Ok(line) = line else {
            tracing::warn!("Could not read line from log file");
            continue;
        };
        // If pulling logs via `aws logs tail`, extra information will be prepended on each line.
        let start_index = line
            .find('{')
            .with_context(|| format!("Line {idx} does not contain {{. {line}"))?;
        let end_index = line
            .rfind('}')
            .with_context(|| format!("Line {idx} does not contain }}. {line}"))?;
        if end_index < start_index {
            anyhow::bail!("Json braces out of order on line {idx}: {line}");
        }
        let json_str = &line[start_index..=end_index];

        // Auto-detect format on first line
        if detected_format.is_none() {
            detected_format = Some(detect_format(json_str));
        }

        #[expect(clippy::missing_panics_doc, reason = "checked above")]
        let result = match detected_format.unwrap() {
            LogFormat::Ffevents => parse_ffevents_line(json_str, idx),
            LogFormat::Primer => {
                match log_format_primer::parse_line(json_str, &mut primer_femto_mode) {
                    Ok(Some(pair)) => Ok(pair),
                    Ok(None) => continue,
                    Err(e) => Err(e),
                }
            }
        };

        match result {
            Ok((timestamp, repro_event)) => events.push((timestamp, repro_event)),
            Err(e) => {
                tracing::warn!("Could not parse line {idx} from log file: {e}");
            }
        }
    }

    if events.is_empty() {
        anyhow::bail!("No events found in log file");
    }

    // Sort by the timestamp
    events.sort_by(|(a_ts, _), (b_ts, _)| a_ts.cmp(b_ts));
    let events = events.into_iter().map(|(_, e)| e).collect();
    Ok(events)
}

/// Detect log format by examining the JSON structure.
///
/// - If the line has a top-level `"fields"` key, it's primer format.
/// - If the line has a top-level `"event"` key with a string value, it's ffevents.
fn detect_format(json_str: &str) -> LogFormat {
    // Quick heuristic: primer format always has "fields" as a top-level key
    if json_str.contains("\"fields\"") {
        LogFormat::Primer
    } else {
        LogFormat::Ffevents
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
                event: feed.input,
                output: feed.output,
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
    input: RoutableEvent,
    output: Option<ClockParameters>,
}

#[cfg(test)]
mod tests {
    use clock_bound::daemon::event::TscRtt;

    use super::*;

    #[test]
    fn parse_ffevents_log() {
        let log = include_str!("test_ffevents.log");
        let events = repro_events_from_reader(log.as_bytes()).unwrap();

        // 1 init + 5 feeds + 1 disruption + 1 feed = 8 lines total
        assert_eq!(events.len(), 8);

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
    fn parse_ffevents_log_with_cloudwatch_prefix() {
        let log = r#"2026-07-16T14:33:49.000000+00:00 i-abc123 {"timestamp":"2026-07-16T14:33:49.654418Z","version":"3.0.0-alpha.2","event":"feed","feed":{"input":{"type":"ntp","identifier":"169.254.169.123:123","counter_pre":482393264419,"counter_post":482393405301,"data":{"server_recv_time_ns":1784212429654307727,"server_send_time_ns":1784212429654319674,"root_delay_ns":30517578125000,"root_dispersion_ns":15258789063000,"stratum":1}},"output":null}}
2026-07-16T14:33:51.000000+00:00 i-abc123 {"timestamp":"2026-07-16T14:33:51.654418Z","version":"3.0.0-alpha.2","event":"feed","feed":{"input":{"type":"phc","identifier":"/dev/ptp0","counter_pre":482398526053,"counter_post":482398528552,"data":{"time_ns":1784212431654498913,"clock_error_bound_ns":21119}},"output":null}}
"#;
        let events = repro_events_from_reader(log.as_bytes()).unwrap();
        assert_eq!(events.len(), 2);
        assert!(matches!(events[0], ReproEvent::Feed { .. }));
        assert!(matches!(events[1], ReproEvent::Feed { .. }));
    }

    #[test]
    fn parse_primer_format_nanos() {
        let log = include_str!("test_11_06_2025_nanos.log");
        let events = repro_events_from_reader(log.as_bytes()).unwrap();

        // 3 of the 13 events are the first input for each source (no output yet).
        // The remaining 10 produce clock parameters.
        let param_count = events
            .iter()
            .filter_map(|event| {
                let ReproEvent::Feed { output, .. } = event else {
                    return None;
                };
                output.as_ref()
            })
            .count();
        assert_eq!(param_count, 10);
    }

    #[test]
    fn parse_primer_format_femtoseconds() {
        let log = include_str!("test_11_06_2025.log");
        let events = repro_events_from_reader(log.as_bytes()).unwrap();

        // Same fixture as nanos but with femtosecond values — should produce identical results
        let param_count = events
            .iter()
            .filter_map(|event| {
                let ReproEvent::Feed { output, .. } = event else {
                    return None;
                };
                output.as_ref()
            })
            .count();
        assert_eq!(param_count, 10);
    }

    #[test]
    fn parse_primer_cloudwatch_logs() {
        let log = include_str!("cw_tail_12_29_2026.log");
        let events = repro_events_from_reader(log.as_bytes()).unwrap();

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

        // verify sorted by counter_pre
        let is_sorted = feed_events.is_sorted_by_key(|e| e.counter_pre());
        assert!(is_sorted);
    }

    #[test]
    fn detect_format_ffevents() {
        let line = r#"{"timestamp":"2026-07-16T14:30:00Z","version":"3.0.0","event":"init","init":{"message":"test"}}"#;
        assert_eq!(detect_format(line), LogFormat::Ffevents);
    }

    #[test]
    fn detect_format_primer() {
        let line = r#"{"timestamp":"2025-11-06T17:23:20Z","level":"INFO","fields":{"message":"feed","event":"{}","output":"null"},"target":"clock_bound::primer"}"#;
        assert_eq!(detect_format(line), LogFormat::Primer);
    }

    #[test]
    fn plaintext_log_file_parses_same_as_reader() {
        let example_log = include_str!("test_11_06_2025.log");
        let expected = repro_events_from_reader(example_log.as_bytes()).unwrap();

        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), example_log).unwrap();

        let events = repro_events_from_log_file(tmp.path()).unwrap();
        assert!(!events.is_empty());
        assert_eq!(format!("{events:?}"), format!("{expected:?}"));
    }

    #[test]
    fn gzip_log_file_round_trips() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write;

        let example_log = include_str!("test_11_06_2025.log");
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

        let events = repro_events_from_log_file(tmp.path()).unwrap();
        assert!(!events.is_empty());
        assert_eq!(format!("{events:?}"), format!("{expected:?}"));
    }
}
