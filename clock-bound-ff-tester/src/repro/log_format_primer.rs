//! Primer log format parser
//!
//! Parses the old `primer.log` format produced by ClockBound ≤2.x.
//! This format used tracing-subscriber's JSON formatter with escaped inner strings.
//!
//! # TODO
//!
//! Re-evaluate the necessity of this backward compatibility module. Once all existing
//! primer.log captures have been replayed or archived, this module can be removed.

use anyhow::Context;
use clock_bound::daemon::{
    clock_parameters,
    clock_sync_algorithm::source::DevicePath,
    event::{self},
    receiver_stream,
    time::{Duration, Instant, TscCount, tsc::Period},
};
use std::net::SocketAddr;

use super::ReproEvent;

/// Any time field exceeding this value cannot be a valid nanosecond timestamp.
/// Corresponds to roughly year 2535 in nanoseconds. Values above this in the first
/// feed event indicate we are reading the legacy femtosecond format.
const FEMTOSECOND_THRESHOLD: i128 = 17_828_542_000_000_000_000;

/// Conversion factor from femtoseconds to nanoseconds
const FEMTOS_PER_NANO: i128 = 1_000_000;

/// Time-valued field names in event JSON that need femtosecond conversion
const EVENT_TIME_FIELDS: &[&str] = &[
    "server_recv_time",
    "server_send_time",
    "root_delay",
    "root_dispersion",
    "clock_error_bound",
    "time",
];

/// Time-valued field names in output JSON that need femtosecond conversion
const OUTPUT_TIME_FIELDS: &[&str] = &["time", "clock_error_bound"];

/// Parse a single primer-format JSON line into a `ReproEvent`.
///
/// Returns `Ok(None)` if the line should be skipped (unparseable but non-fatal).
pub fn parse_line(
    json_str: &str,
    femtosecond_mode: &mut Option<bool>,
) -> anyhow::Result<Option<(String, ReproEvent)>> {
    let line: Line =
        serde_json::from_str(json_str).with_context(|| "Failed to parse as primer format")?;

    let repro_event = match &line.fields {
        FieldEnum::Init(_) => ReproEvent::Init,
        FieldEnum::Disruption(_) => ReproEvent::Disruption,
        FieldEnum::Feed(fields) => {
            // Detect femtosecond format on first feed event
            if femtosecond_mode.is_none() {
                let raw_event = fields.event.replace("\\\"", "\"");
                *femtosecond_mode = Some(detect_femtosecond_format(&raw_event));
            }
            let is_femto = femtosecond_mode.unwrap_or(false);

            let event = parse_primer_event(&fields.event, is_femto)?;
            let output = parse_primer_output(&fields.output, is_femto)?;

            ReproEvent::Feed { event, output }
        }
    };

    Ok(Some((line.timestamp, repro_event)))
}

/// Detect if a raw event JSON string uses femtosecond values
fn detect_femtosecond_format(event_json: &str) -> bool {
    for key in ["server_recv_time", "\"time\""] {
        if let Some(value) = extract_numeric_value_after_key(event_json, key) {
            return value > FEMTOSECOND_THRESHOLD;
        }
    }
    false
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

/// Parse a primer-format event string into a `receiver_stream::RoutableEvent`
fn parse_primer_event(
    raw: &str,
    femtosecond: bool,
) -> anyhow::Result<receiver_stream::RoutableEvent> {
    let unescaped = raw.replace("\\\"", "\"");
    let json_str = if femtosecond {
        convert_time_fields(unescaped, EVENT_TIME_FIELDS)
    } else {
        unescaped
    };

    // Deserialize the old enum-tagged format into our local types
    let parsed: RoutableEvent = serde_json::from_str(&json_str)?;
    Ok(parsed.into_current())
}

/// Parse a primer-format output string into `Option<clock_parameters::ClockParameters>`
fn parse_primer_output(
    raw: &str,
    femtosecond: bool,
) -> anyhow::Result<Option<clock_parameters::ClockParameters>> {
    let unescaped = raw.replace("\\\"", "\"");
    let json_str = if femtosecond {
        convert_time_fields(unescaped, OUTPUT_TIME_FIELDS)
    } else {
        unescaped
    };

    let parsed: Option<ClockParameters> = serde_json::from_str(&json_str)?;
    Ok(parsed.map(ClockParameters::into_current))
}

// --- Deserialization types ---

#[derive(serde::Deserialize)]
struct Line {
    timestamp: String,
    fields: FieldEnum,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum FieldEnum {
    Feed(FeedFields),
    Init(#[allow(unused)] Init),
    Disruption(#[allow(unused)] Disruption),
}

#[derive(serde::Deserialize)]
struct Init {
    #[allow(dead_code)]
    init: String,
}

#[derive(serde::Deserialize)]
struct Disruption {
    #[allow(dead_code)]
    disruption: String,
}

#[derive(serde::Deserialize)]
struct FeedFields {
    event: String,
    output: String,
}

/// `RoutableEvent` in the primer serialization shape (enum-tagged)
#[derive(serde::Deserialize)]
enum RoutableEvent {
    #[serde(alias = "LinkLocal")]
    AmazonTimeSync(Ntp),
    NtpSource(SocketAddr, Ntp),
    Phc(Phc),
}

impl RoutableEvent {
    fn into_current(self) -> receiver_stream::RoutableEvent {
        match self {
            Self::AmazonTimeSync(ntp) => {
                let event = ntp.into_event();
                receiver_stream::RoutableEvent::AmazonTimeSync(event)
            }
            Self::NtpSource(addr, ntp) => {
                let event = ntp.into_event();
                receiver_stream::RoutableEvent::NtpSource(addr, event)
            }
            Self::Phc(phc) => {
                let event = phc.into_event();
                // The legacy primer format did not record a PHC device path;
                // default to the conventional device so replayed events carry a
                // stable identifier.
                receiver_stream::RoutableEvent::Phc(DevicePath::from("/dev/ptp0"), event)
            }
        }
    }
}

#[derive(serde::Deserialize)]
struct Ntp {
    #[serde(alias = "tsc_pre")]
    counter_pre: i64,
    #[serde(alias = "tsc_post")]
    counter_post: i64,
    data: NtpData,
}

impl Ntp {
    fn into_event(self) -> event::Ntp {
        event::Ntp::builder()
            .counter_pre(TscCount::new(self.counter_pre))
            .counter_post(TscCount::new(self.counter_post))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::new(self.data.server_recv_time),
                server_send_time: Instant::new(self.data.server_send_time),
                root_delay: Duration::new(self.data.root_delay),
                root_dispersion: Duration::new(self.data.root_dispersion),
                stratum: event::Stratum::new(self.data.stratum).unwrap_or(event::Stratum::ONE),
            })
            .build()
            .expect("invalid NTP event in primer log: counter_post <= counter_pre")
    }
}

#[derive(serde::Deserialize)]
struct NtpData {
    server_recv_time: i64,
    server_send_time: i64,
    root_delay: i64,
    root_dispersion: i64,
    stratum: u8,
}

#[derive(serde::Deserialize)]
struct Phc {
    #[serde(alias = "tsc_pre")]
    counter_pre: i64,
    #[serde(alias = "tsc_post")]
    counter_post: i64,
    data: PhcData,
}

impl Phc {
    fn into_event(self) -> event::Phc {
        event::Phc::builder()
            .counter_pre(TscCount::new(self.counter_pre))
            .counter_post(TscCount::new(self.counter_post))
            .data(event::PhcData {
                time: Instant::new(self.data.time),
                clock_error_bound: Duration::new(self.data.clock_error_bound),
            })
            .build()
            .expect("invalid PHC event in primer log: counter_post <= counter_pre")
    }
}

#[derive(serde::Deserialize)]
struct PhcData {
    time: i64,
    clock_error_bound: i64,
}

/// `ClockParameters` with primer-era field names
#[derive(serde::Deserialize)]
struct ClockParameters {
    tsc_count: i64,
    time: i64,
    clock_error_bound: i64,
    period: f64,
    period_max_error: f64,
}

impl ClockParameters {
    fn into_current(self) -> clock_parameters::ClockParameters {
        clock_parameters::ClockParameters {
            tsc_count: TscCount::new(self.tsc_count),
            time: Instant::new(self.time),
            clock_error_bound: Duration::new(self.clock_error_bound),
            period: Period::from_seconds(self.period),
            period_max_error: Period::from_seconds(self.period_max_error),
            as_of_monotonic: Instant::new(0),
        }
    }
}

/// Convert time-valued fields in a JSON string from femtoseconds to nanoseconds
fn convert_time_fields(mut json: String, fields: &[&str]) -> String {
    for &field in fields {
        let search_key = format!("\"{field}\"");
        let mut search_start = 0;

        while let Some(key_pos) = json[search_start..].find(&search_key) {
            let key_pos = search_start + key_pos;
            let after_key = key_pos + search_key.len();

            let Some(colon_offset) = json[after_key..].find(':') else {
                break;
            };
            let colon_pos = after_key + colon_offset;

            let value_start = json[colon_pos + 1..]
                .find(|c: char| !c.is_whitespace())
                .map_or(colon_pos + 1, |offset| colon_pos + 1 + offset);

            let first_char = json.as_bytes().get(value_start).copied().unwrap_or(0);
            if !first_char.is_ascii_digit() && first_char != b'-' {
                search_start = value_start;
                continue;
            }

            let value_end = json[value_start..]
                .find(|c: char| !c.is_ascii_digit() && c != '-')
                .map_or(json.len(), |offset| value_start + offset);

            let num_str = &json[value_start..value_end];

            if json[value_end..].starts_with('.') || json[value_end..].starts_with('e') {
                search_start = value_end;
                continue;
            }

            if let Ok(value) = num_str.parse::<i128>() {
                let converted = value / FEMTOS_PER_NANO;
                let converted_str = converted.to_string();
                json.replace_range(value_start..value_end, &converted_str);
                search_start = value_start + converted_str.len();
            } else {
                search_start = value_end;
            }
        }
    }
    json
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ntp_event() {
        let raw = r#"{"LinkLocal":{"tsc_pre":635623413319024,"tsc_post":635623413864816,"data":{"server_recv_time":1762449800677131039,"server_send_time":1762449800677146186,"root_delay":30517,"root_dispersion":15258,"stratum":1}}}"#;
        let event = parse_primer_event(raw, false).unwrap();
        let receiver_stream::RoutableEvent::AmazonTimeSync(ntp) = &event else {
            panic!("expected AmazonTimeSync");
        };
        assert_eq!(ntp.counter_pre().get(), 635_623_413_319_024);
        assert_eq!(ntp.data().server_recv_time.get(), 1_762_449_800_677_131_039);
    }

    #[test]
    fn parse_phc_event() {
        let raw = r#"{"Phc":{"counter_pre":302084682318463,"counter_post":302084682325383,"data":{"time":1766995198825456232,"clock_error_bound":23239}}}"#;
        let event = parse_primer_event(raw, false).unwrap();
        let receiver_stream::RoutableEvent::Phc(_, phc) = &event else {
            panic!("expected Phc");
        };
        assert_eq!(phc.counter_pre().get(), 302_084_682_318_463);
        assert_eq!(phc.data().time.get(), 1_766_995_198_825_456_232);
    }

    #[test]
    fn parse_femtosecond_ntp() {
        let raw = r#"{"LinkLocal":{"tsc_pre":635623413319024,"tsc_post":635623413864816,"data":{"server_recv_time":1762449800677131039000000,"server_send_time":1762449800677146186000000,"root_delay":30517578125,"root_dispersion":15258789063,"stratum":1}}}"#;
        let event = parse_primer_event(raw, true).unwrap();
        let receiver_stream::RoutableEvent::AmazonTimeSync(ntp) = &event else {
            panic!("expected AmazonTimeSync");
        };
        assert_eq!(ntp.counter_pre().get(), 635_623_413_319_024);
        assert_eq!(ntp.data().server_recv_time.get(), 1_762_449_800_677_131_039);
        assert_eq!(ntp.data().root_delay.get(), 30_517);
    }

    #[test]
    fn parse_output() {
        let raw = r#"{"tsc_count":302084682321923,"time":1766995198825455884,"clock_error_bound":27093,"period":9.523718455638937e-10,"period_max_error":5.07759648309667e-16}"#;
        let output = super::parse_primer_output(raw, false).unwrap().unwrap();
        assert_eq!(output.tsc_count.get(), 302_084_682_321_923);
        assert_eq!(output.time.get(), 1_766_995_198_825_455_884);
        assert_eq!(output.clock_error_bound.get(), 27_093);
    }

    #[test]
    fn detect_femtosecond() {
        let nano = r#"{"server_recv_time":1762449800677131039}"#;
        assert!(!detect_femtosecond_format(nano));

        let femto = r#"{"server_recv_time":1762449800677131039000000}"#;
        assert!(detect_femtosecond_format(femto));
    }

    #[test]
    fn full_primer_line() {
        let line = r#"{"timestamp":"2025-11-06T17:23:20.677297Z","level":"INFO","fields":{"message":"feed","event":"{\"LinkLocal\":{\"tsc_pre\":635623413319024,\"tsc_post\":635623413864816,\"data\":{\"server_recv_time\":1762449800677131039,\"server_send_time\":1762449800677146186,\"root_delay\":30517,\"root_dispersion\":15258,\"stratum\":1}}}","output":"null"},"target":"clock_bound::primer"}"#;
        let mut femto_mode = None;
        let result = parse_line(line, &mut femto_mode).unwrap().unwrap();
        assert_eq!(result.0, "2025-11-06T17:23:20.677297Z");
        assert!(matches!(result.1, ReproEvent::Feed { output: None, .. }));
    }
}
