//! Functions and structs related to parsing the primer logfiles

use anyhow::Context;
use flate2::read::MultiGzDecoder;

use clock_bound::daemon::{clock_parameters::ClockParameters, receiver_stream::RoutableEvent};

use super::ReproEvent;
use super::legacy_femtoseconds;

use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

/// Any time field exceeding this value cannot be a valid nanosecond timestamp.
/// Corresponds to roughly year 2535 in nanoseconds. Values above this in the first
/// feed event indicate we are reading the legacy femtosecond format.
const FEMTOSECOND_THRESHOLD: i128 = 17_828_542_000_000_000_000;

/// The detected log format for a given log file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Current format: i64 nanoseconds (previously was i128 femtoseconds)
    Current,
    /// Legacy format: i128 femtoseconds
    Femtoseconds,
}

impl LogFormat {
    /// Detect the log format from a raw event JSON string
    ///
    /// Scans for `server_recv_time` or `"time"` key and checks whether
    /// its value exceeds the femtosecond threshold.
    pub fn detect_from_event_str(event_json: &str) -> Self {
        for key in ["server_recv_time", "\"time\""] {
            if let Some(value) = extract_numeric_value_after_key(event_json, key) {
                if value > FEMTOSECOND_THRESHOLD {
                    return Self::Femtoseconds;
                }
                return Self::Current;
            }
        }
        // Default to current format if we can't detect
        Self::Current
    }
}

/// Extract a numeric value that follows a given key in a JSON string
fn extract_numeric_value_after_key(json: &str, key: &str) -> Option<i128> {
    let idx = json.find(key)?;
    let after_key = &json[idx + key.len()..];
    let colon_idx = after_key.find(':')?;
    let after_colon = after_key[colon_idx + 1..].trim_start();
    let num_str: String = after_colon
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '-')
        .collect();
    num_str.parse::<i128>().ok()
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
/// - Returns and error if unable to read values from the logfile
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

/// Convert a log file at `file_path` into a `ff-tester` scenario
///
/// Returns a Scenario, and the output from every call into `ClockSyncAlgorithm::feed_amazon_time_sync`
///
/// Automatically detects whether the log uses the legacy femtosecond format or the
/// current nanosecond format by inspecting the first feed event. Legacy values are
/// converted to nanoseconds transparently.
///
/// # Errors
/// Returns errors if the logfile is corrupted
pub fn repro_events_from_reader(input: impl Read) -> anyhow::Result<Vec<ReproEvent>> {
    let reader = BufReader::new(input);

    let mut events = Vec::new();
    let mut detected_format: Option<LogFormat> = None;

    for (idx, line) in reader.lines().enumerate() {
        let Ok(line) = line else {
            tracing::warn!("Could not read line from log file");
            continue;
        };
        // If pulling logs via `aws logs tail`, extra information will be prepended on each line. Strip it out
        let start_index = line
            .find('{')
            .with_context(|| format!("Line {idx} does contain {{. {line}"))?;
        let end_index = line
            .rfind('}')
            .with_context(|| format!("Line {idx} does not contain }} {line}"))?;
        if end_index < start_index {
            anyhow::bail!("Json braces out of order on line {idx}: {line}");
        }
        let line = &line[start_index..=end_index];
        let line: Line = match serde_json::from_str(line) {
            Ok(line) => line,
            Err(e) => {
                tracing::warn!(line, "Could not parse line from log file: {e}");
                continue;
            }
        };

        let repro_event = match &line.fields {
            FieldEnum::Init(_init) => ReproEvent::Init,

            FieldEnum::Disruption(_disruption) => ReproEvent::Disruption,

            FieldEnum::FeedEvent(fields) => {
                // Detect the log format on the first feed event
                if detected_format.is_none() {
                    let raw_event = fields.event.replace("\\\"", "\"");
                    detected_format = Some(LogFormat::detect_from_event_str(&raw_event));
                    if detected_format == Some(LogFormat::Femtoseconds) {
                        tracing::info!(
                            "Detected legacy femtosecond log format, converting to nanoseconds"
                        );
                    }
                }

                let format = detected_format.unwrap_or(LogFormat::Current);

                let event = match fields.parse_event(format) {
                    Ok(event) => event,
                    Err(e) => {
                        tracing::warn!(line = ?line, "Could not parse event: {e}");
                        continue;
                    }
                };

                let output = match fields.parse_output(format) {
                    Ok(output) => output,
                    Err(e) => {
                        tracing::warn!(line = ?line, "Could not parse output: {e}");
                        continue;
                    }
                };

                ReproEvent::Feed { event, output }
            }
        };
        events.push((line.timestamp, repro_event));
    }

    if events.is_empty() {
        anyhow::bail!("No events found in log file");
    }

    // Sort by the timestamp
    events.sort_by(|(a_ts, _), (b_ts, _)| a_ts.cmp(b_ts));

    let events = events.into_iter().map(|(_, e)| e).collect();

    Ok(events)
}

/// Convenience struct to parse each line of the log
///
/// It's not exhaustive, but has the minimum number of fields
/// to get the [`NTP event`](clock_bound::daemon::event::Ntp) and [`ClockParameters`]
#[derive(Debug, Clone, serde::Deserialize)]
struct Line {
    /// The timestamp when the log was written
    ///
    /// Need this value because a misconfigured cloudwatch can store logs in the incorrect order.
    /// And we can use this to sort the logs in this case.
    ///
    /// Using a string since it's in ISO 8601 format (YYYY-MM-DD HH:MM) which sorts alphabetically
    timestamp: String,
    /// The actual fields of the log
    fields: FieldEnum,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(untagged)] // load bearing untagged
enum FieldEnum {
    FeedEvent(Fields),
    Init(Init),
    Disruption(Disruption),
}

#[derive(Debug, Clone, serde::Deserialize)]
#[expect(unused, reason = "key used for deserialization")]
struct Init {
    init: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[expect(unused, reason = "key used for deserialization")]
struct Disruption {
    disruption: String,
}

/// Workaround for tracing escapes
///
/// Holds strings to make it easy to grab the inner keys. But since these values cannot be parsed directly from json,
/// implement methods to parse each event.
///
/// tracing currently escapes double quote characters in json when logging. So we need to convert all `\"` into '"'
#[derive(Debug, Clone, serde::Deserialize)]
struct Fields {
    event: String,
    output: String,
}

impl Fields {
    fn parse_event(&self, format: LogFormat) -> anyhow::Result<RoutableEvent> {
        let unescaped = self.event.replace("\\\"", "\"");
        match format {
            LogFormat::Femtoseconds => legacy_femtoseconds::parse_legacy_event(unescaped),
            LogFormat::Current => {
                let retval: RoutableEvent = serde_json::from_str(&unescaped)?;
                Ok(retval)
            }
        }
    }

    fn parse_output(&self, format: LogFormat) -> anyhow::Result<Option<ClockParameters>> {
        let unescaped = self.output.replace("\\\"", "\"");
        match format {
            LogFormat::Femtoseconds => legacy_femtoseconds::parse_legacy_output(unescaped),
            LogFormat::Current => {
                let retval: Option<ClockParameters> = serde_json::from_str(&unescaped)?;
                Ok(retval)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use clock_bound::daemon::event::TscRtt;

    use super::*;

    #[test]
    fn detect_current_format() {
        let event_str = r#"{"LinkLocal":{"counter_pre":635623413319024,"counter_post":635623413864816,"data":{"server_recv_time":1762449800677131039,"server_send_time":1762449800677146186,"root_delay":30517,"root_dispersion":15258,"stratum":1}}}"#;
        assert_eq!(
            LogFormat::detect_from_event_str(event_str),
            LogFormat::Current
        );
    }

    #[test]
    fn detect_femtosecond_format() {
        let event_str = r#"{"LinkLocal":{"counter_pre":635623413319024,"counter_post":635623413864816,"data":{"server_recv_time":1762449800677131039000000,"server_send_time":1762449800677146186000000,"root_delay":30517578125,"root_dispersion":15258789063,"stratum":1}}}"#;
        assert_eq!(
            LogFormat::detect_from_event_str(event_str),
            LogFormat::Femtoseconds
        );
    }

    #[test]
    fn detect_phc_femtosecond_format() {
        let event_str = r#"{"Phc":{"counter_pre":302084682318463,"counter_post":302084682325383,"data":{"time":1766995198825456232000000,"clock_error_bound":23239000000}}}"#;
        assert_eq!(
            LogFormat::detect_from_event_str(event_str),
            LogFormat::Femtoseconds
        );
    }

    #[test]
    fn current_format_event_parses_directly() {
        let input = r#"{"LinkLocal":{"counter_pre":635623413319024,"counter_post":635623413864816,"data":{"server_recv_time":1762449800677131039,"server_send_time":1762449800677146186,"root_delay":30517,"root_dispersion":15258,"stratum":1}}}"#;
        assert_eq!(LogFormat::detect_from_event_str(input), LogFormat::Current);
        let _event: RoutableEvent = serde_json::from_str(input).unwrap();
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

    #[test]
    fn scenario_from_logs() {
        let example_log = include_str!("test_11_06_2025.log");

        let events = repro_events_from_reader(example_log.as_bytes()).unwrap();

        let param_count = events
            .iter()
            .filter_map(|event| {
                let ReproEvent::Feed {
                    event: _event,
                    output: params,
                } = event
                else {
                    return None;
                };
                params.as_ref()
            })
            .count();
        assert_eq!(param_count, 10); // 3 of the values are the first inputs for sources. So 10 calculate clock parameters
    }

    #[test]
    fn scenario_from_cloudwatch_logs() {
        // this log also has an out of order log event
        let log = include_str!("cw_tail_12_29_2026.log");

        let events = repro_events_from_reader(log.as_bytes()).unwrap();

        let events: Vec<_> = events
            .iter()
            .filter_map(|event| {
                let ReproEvent::Feed {
                    event,
                    output: _params,
                } = event
                else {
                    return None;
                };
                Some(event)
            })
            .collect();

        assert_eq!(events.len(), 13);

        // verify that it's deserialized and sorted
        let is_sorted = events.is_sorted_by_key(|e| e.counter_pre());
        assert!(is_sorted);
    }
}
