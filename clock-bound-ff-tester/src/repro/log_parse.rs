//! Functions and structs related to parsing the primer logfiles

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

    for line in reader.lines() {
        let Ok(line) = line else {
            tracing::warn!("Could not read line from log file");
            continue;
        };
        let line: Line = match serde_json::from_str(&line) {
            Ok(line) => line,
            Err(e) => {
                tracing::warn!(line, "Could not parse line from log file: {e}");
                continue;
            }
        };

        match &line.fields {
            FieldEnum::Init(_init) => events.push(ReproEvent::Init),

            FieldEnum::Disruption(_disruption) => events.push(ReproEvent::Disruption),

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

                events.push(ReproEvent::Feed { event, output });
            }
        }
    }

    if events.is_empty() {
        anyhow::bail!("No events found in log file");
    }

    Ok(events)
}

/// Convenience struct to parse each line of the log
///
/// It's not exhaustive, but has the minimum number of fields
/// to get the [`NTP event`](clock_bound::daemon::event::Ntp) and [`ClockParameters`]
#[derive(Debug, Clone, serde::Deserialize)]
struct Line {
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
}
