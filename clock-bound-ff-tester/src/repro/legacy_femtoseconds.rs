//! Legacy log format support
//!
//! Prior to the nanosecond time type refactor, ClockBound logged time values as i128 femtoseconds.
//! This module detects those logs and converts them to the current nanosecond-based format.
//!
//! Detection heuristic: if `server_recv_time` or `time` (PHC) fields exceed
//! [`FEMTOSECOND_THRESHOLD`], the value must be in the old femtosecond format.
//! Any real nanosecond timestamp that large would be centuries in the future.
//!
//! Because the old femtosecond values exceed `i64::MAX`, they cannot be deserialized
//! directly into the current `Instant`/`Duration` types. Instead, we transform the
//! raw JSON strings by dividing time-valued fields by [`FEMTOS_PER_NANO`] before
//! passing them to serde.

use clock_bound::daemon::{clock_parameters::ClockParameters, receiver_stream::RoutableEvent};

/// Conversion factor from femtoseconds to nanoseconds
const FEMTOS_PER_NANO: i128 = 1_000_000;

/// Time-valued field names in event JSON that need conversion
const EVENT_TIME_FIELDS: &[&str] = &[
    "server_recv_time",
    "server_send_time",
    "root_delay",
    "root_dispersion",
    "clock_error_bound",
    "time",
];

/// Time-valued field names in output (`ClockParameters`) JSON that need conversion.
/// Note: `tsc_count` is NOT a time value, and `period`/`period_max_error` are f64 rates.
const OUTPUT_TIME_FIELDS: &[&str] = &["time", "clock_error_bound"];

/// Parse a legacy femtosecond event string into a [`RoutableEvent`]
///
/// Converts the JSON to nanosecond format first, then deserializes.
pub fn parse_legacy_event(raw_event: String) -> anyhow::Result<RoutableEvent> {
    let converted = convert_time_fields(raw_event, EVENT_TIME_FIELDS);
    let event: RoutableEvent = serde_json::from_str(&converted)?;
    Ok(event)
}

/// Parse a legacy femtosecond output string into [`Option<ClockParameters>`]
///
/// Converts the JSON to nanosecond format first, then deserializes.
pub fn parse_legacy_output(raw_output: String) -> anyhow::Result<Option<ClockParameters>> {
    let converted = convert_time_fields(raw_output, OUTPUT_TIME_FIELDS);
    let output: Option<ClockParameters> = serde_json::from_str(&converted)?;
    Ok(output)
}

/// Convert all occurrences of time-valued fields in a JSON string from femtoseconds to nanoseconds
///
/// For each field name in `fields`, finds patterns like `"field_name":12345` and replaces the
/// numeric value with `value / FEMTOS_PER_NANO`.
fn convert_time_fields(mut json: String, fields: &[&str]) -> String {
    for &field in fields {
        // Build the pattern: `"field_name"` followed by `:` and a number
        let search_key = format!("\"{field}\"");
        let mut search_start = 0;

        while let Some(key_pos) = json[search_start..].find(&search_key) {
            let key_pos = search_start + key_pos;
            let after_key = key_pos + search_key.len();

            // Find the colon
            let Some(colon_offset) = json[after_key..].find(':') else {
                break;
            };
            let colon_pos = after_key + colon_offset;

            // Skip whitespace after colon
            let value_start = json[colon_pos + 1..]
                .find(|c: char| !c.is_whitespace())
                .map_or(colon_pos + 1, |offset| colon_pos + 1 + offset);

            // Check if what follows is a number (not a float like period values)
            let first_char = json.as_bytes().get(value_start).copied().unwrap_or(0);
            if !first_char.is_ascii_digit() && first_char != b'-' {
                search_start = value_start;
                continue;
            }

            // Extract the numeric string
            let value_end = json[value_start..]
                .find(|c: char| !c.is_ascii_digit() && c != '-')
                .map_or(json.len(), |offset| value_start + offset);

            let num_str = &json[value_start..value_end];

            // Skip if it looks like a float (shouldn't happen for integer fields, but be safe)
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
    fn convert_ntp_event_json() {
        let input = r#"{"LinkLocal":{"tsc_pre":635623413319024,"tsc_post":635623413864816,"data":{"server_recv_time":1762449800677131039000000,"server_send_time":1762449800677146186000000,"root_delay":30517578125,"root_dispersion":15258789063,"stratum":1}}}"#;
        let event: RoutableEvent =
            serde_json::from_str(&convert_time_fields(input.to_owned(), EVENT_TIME_FIELDS))
                .unwrap();

        let RoutableEvent::LinkLocal(ntp) = &event else {
            panic!("expected LinkLocal");
        };
        assert_eq!(ntp.tsc_pre().get(), 635_623_413_319_024);
        assert_eq!(ntp.data().server_recv_time.get(), 1_762_449_800_677_131_039);
        assert_eq!(ntp.data().root_delay.get(), 30_517);
    }

    #[test]
    fn convert_phc_event_json() {
        let input = r#"{"Phc":{"tsc_pre":302084682318463,"tsc_post":302084682325383,"data":{"time":1766995198825456232000000,"clock_error_bound":23239000000}}}"#;
        let event: RoutableEvent =
            serde_json::from_str(&convert_time_fields(input.to_owned(), EVENT_TIME_FIELDS))
                .unwrap();

        let RoutableEvent::Phc(phc) = &event else {
            panic!("expected Phc");
        };
        assert_eq!(phc.tsc_pre().get(), 302_084_682_318_463);
        assert_eq!(phc.data().time.get(), 1_766_995_198_825_456_232);
        assert_eq!(phc.data().clock_error_bound.get(), 23_239);
    }

    #[test]
    fn convert_output_json() {
        let input = r#"{"tsc_count":302084682321923,"time":1766995198825455884755430,"clock_error_bound":27093915940,"period":9.523718455638937e-10,"period_max_error":5.07759648309667e-16}"#;
        let params: ClockParameters =
            serde_json::from_str(&convert_time_fields(input.to_owned(), OUTPUT_TIME_FIELDS))
                .unwrap();
        assert_eq!(params.tsc_count.get(), 302_084_682_321_923);
        assert_eq!(params.time.get(), 1_766_995_198_825_455_884);
        assert_eq!(params.clock_error_bound.get(), 27_093);
    }

    #[test]
    fn parse_legacy_event_end_to_end() {
        let input = r#"{"LinkLocal":{"tsc_pre":635623413319024,"tsc_post":635623413864816,"data":{"server_recv_time":1762449800677131039000000,"server_send_time":1762449800677146186000000,"root_delay":30517578125,"root_dispersion":15258789063,"stratum":1}}}"#;
        let event = parse_legacy_event(input.to_owned()).unwrap();

        let RoutableEvent::LinkLocal(ntp) = &event else {
            panic!("expected LinkLocal");
        };
        // TSC values unchanged
        assert_eq!(ntp.tsc_pre().get(), 635_623_413_319_024);
        // Time values divided by 1_000_000
        assert_eq!(ntp.data().server_recv_time.get(), 1_762_449_800_677_131_039);
    }
}
