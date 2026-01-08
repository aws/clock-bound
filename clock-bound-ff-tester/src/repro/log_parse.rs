//! Functions and structs related to parsing the primer logfiles

use anyhow::Context;
use clock_bound::daemon::{clock_parameters::ClockParameters, receiver_stream::RoutableEvent};

use super::ReproEvent;

use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::Path,
};

/// Read a logfile and return all inputs and outputs from the `ClockSyncAlgorithm`
///
/// # Errors
/// - Returns an error if unable to open the file.
/// - Returns and error if unable to read values from the logfile
pub fn repro_events_from_log_file(file_path: impl AsRef<Path>) -> anyhow::Result<Vec<ReproEvent>> {
    let file = File::open(file_path)?;
    repro_events_from_reader(file)
}

/// Convert a log file at `file_path` into a `ff-tester` scenario
///
/// Returns a Scenario, and the output from every call into `ClockSyncAlgorithm::feed_link_local`
///
/// # Errors
/// Returns errors if the logfile is corrupted
pub fn repro_events_from_reader(input: impl Read) -> anyhow::Result<Vec<ReproEvent>> {
    let reader = BufReader::new(input);

    let mut events = Vec::new();

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
                let event = match fields.parse_event() {
                    Ok(event) => event,
                    Err(e) => {
                        tracing::warn!(line = ?line, "Could not parse event: {e}");
                        continue;
                    }
                };

                let output = match fields.parse_output() {
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
    fn parse_output(&self) -> anyhow::Result<Option<ClockParameters>> {
        let unescaped = self.output.replace("\\\"", "\"");
        let retval: Option<ClockParameters> = serde_json::from_str(&unescaped)?;
        Ok(retval)
    }

    fn parse_event(&self) -> anyhow::Result<RoutableEvent> {
        let unescaped = self.event.replace("\\\"", "\"");
        let retval: RoutableEvent = serde_json::from_str(&unescaped)?;
        Ok(retval)
    }
}

#[cfg(test)]
mod tests {
    use clock_bound::daemon::event::TscRtt;

    use super::*;

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
        let is_sorted = events.is_sorted_by_key(|e| e.tsc_pre());
        assert!(is_sorted);
    }
}
