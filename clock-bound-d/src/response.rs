// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
use crate::ceb::ClockErrorBound;
use crate::chrony_poller::LEAP_STATUS_UNSYNCHRONIZED;
#[cfg(not(test))]
use crate::tracking::update_root_dispersion;
use byteorder::{ByteOrder, NetworkEndian, WriteBytesExt};
#[cfg(not(test))]
use chrono::Utc;
use chrony_candm::reply::Tracking;
use log::error;

/// The current ClockBound protocol version
pub const RESPONSE_VERSION: u8 = 1;

/// An Error Response to include in the header
pub const ERROR_RESPONSE: u8 = 0;

/// Validate a request.
///
/// Checks if the protocol versions between the client and daemon match.
/// Checks if the request type is valid.
/// Checks if the request is the correct size.
///
/// # Arguments
///
/// * `request_version` - The version of the ClockBound protocol the request is using.
/// * `request_type` - The request type: Error (0), Now (1), Before (2), After (3).
/// * `request_size` - The amount of bytes read from a request received from a client.
pub fn validate_request(request_version: u8, request_type: u8, request_size: usize) -> bool {
    // Validate request version
    if request_version != RESPONSE_VERSION {
        error!(
            "The request version {} does not match the response version {}",
            request_version, RESPONSE_VERSION
        );
        return false;
    }

    // Validate request type and size
    return match request_type {
        1 => {
            // Now request. A now request should be the size of just a header which is 4 bytes.
            if request_size == 4 {
                return true;
            }
            error!(
                "Received Now request with invalid size. Expected: 4 bytes, Received: {} bytes",
                request_size
            );
            false
        }
        2 => {
            // Before request. A before request should have 12 bytes.
            // 4 bytes in the header and 8 in the body.
            if request_size == 12 {
                return true;
            }
            error!(
                "Received Before request with invalid size. Expected: 12 bytes, Received: {} bytes",
                request_size
            );
            false
        }
        3 => {
            // After request. A after request should have 12 bytes.
            // 4 bytes in the header and 8 in the body.
            if request_size == 12 {
                return true;
            }
            error!(
                "Received After request with invalid size. Expected: 12 bytes, Received: {} bytes",
                request_size
            );
            false
        }
        _ => {
            // Any other request is invalid.
            error!(
                "Received invalid request type. Valid types: 1 (Now), 2 (Before), 3 (After), Received: {}",
                request_type
            );
            false
        }
    };
}

/// Build a response to send to a client.
///
/// # Arguments
///
/// * `request` - The request received from a client.
/// * `request_size` - The amount of bytes read from a request received from a client.
/// * `tracking` - The tracking information received from Chrony.
/// * `error_flag` - An error flag indicating if there has been an error when getting the tracking
/// information from Chrony.
/// * `max_clock_error` - The assumed maximum frequency error that a system clock can gain between updates in ppm.
pub fn build_response(
    request: [u8; 12],
    request_size: usize,
    tracking: Tracking,
    error_flag: bool,
    max_clock_error: f64,
) -> Vec<u8> {
    // The protocol version of the request
    let request_version = request[0];
    // The request type
    let request_type = request[1];

    // The 3rd and 4th bytes are reserved

    // Chrony tracking information provides the leap status which can be one of four values:
    // Normal (0), Insert second (1), Delete second (2), or Not synchronised (3)
    // If leap status reports as "Not synchronised" then that means Chrony is not synchronised to a
    // source. A client will want to know if Chrony is synchronised or not so set the sync flag
    // false (1) if unsynchronized; otherwise, true (0).
    let sync_flag: u8 = match tracking.leap_status {
        LEAP_STATUS_UNSYNCHRONIZED => 1, // False
        _ => 0,                          // True
    };

    // Don't update the root dispersion when building a response from a test. Updating the root
    // dispersion incorporates checking the current system time which is difficult to test against.
    // Updating root dispersion is tested in its own standalone test.
    #[cfg(not(test))]
    // If there is an issue updating the root dispersion then send back the header only with the
    // response type as error.
    // Even if chronyd is not running the root dispersion will grow based on the last tracking
    // data received from chronyd. Clients can handle this case by seeing that the error flag is
    // set to true due to not being able to get tracking data from chronyd.
    let tracking = match update_root_dispersion(tracking, max_clock_error) {
        Ok(t) => t,
        Err(e) => {
            error!("Root dispersion could not be updated. {:?}", e);
            // If updating the root dispersion fails, then send back only a header with the
            // response type as Error (0).
            return build_response_header(ERROR_RESPONSE, sync_flag);
        }
    };

    let is_valid_request = validate_request(request_version, request_type, request_size);

    // If the error flag is true or if the version of the request does not match the response
    // version; set the response type to Error (0)
    let response_header = if error_flag || !is_valid_request {
        build_response_header(ERROR_RESPONSE, sync_flag)
    } else {
        build_response_header(request_type, sync_flag)
    };

    let ceb_data = ClockErrorBound::from(tracking);

    // Build response based on Request Type
    // Invalid type = Error Response
    // 1 = Now
    // 2 = Before
    // 3 = After
    return match request_type {
        1 => build_response_now(response_header, &ceb_data),
        2 | 3 => {
            // If our request is a before (2) or after (3) request then a body is expected
            let request_body = NetworkEndian::read_u64(&request[4..12]);
            build_response_before_after(response_header, &ceb_data, request_body)
        }
        _ => {
            // If invalid request type then send back the header. The header will return a request
            // type of 0 to indicate an error.
            response_header
        }
    };
}

/// Builds the header of a response.
///
/// # Arguments:
///
/// * `request_type` - The request type: Error (0), Now (1), Before (2), After (3).
/// * `sync_flag` - A flag indicating if Chrony is synchronized to a source. This flag is set based
/// on the leap status value from Chrony's tracking data. If the value is reported as unsynchronized
/// then this flag gets set to false. Otherwise, true.
fn build_response_header(mut request_type: u8, sync_flag: u8) -> Vec<u8> {
    let mut response: Vec<u8> = Vec::with_capacity(4);
    // Send back the response version of ClockBoundD
    response.push(RESPONSE_VERSION);

    // If the request type is not a valid type then set it to Error (0)
    let request_types: [u8; 4] = [0, 1, 2, 3];
    if !request_types.contains(&request_type) {
        request_type = ERROR_RESPONSE;
    }

    // Send back the request type. Will be set to Error (0) if an error occurred.
    response.push(request_type);
    // Set the sync flag based on the Chrony tracking information
    response.push(sync_flag);
    // 4th byte is currently reserved
    let reserved: u8 = 0;
    response.push(reserved);
    response
}

/// Builds the response of a now request to send to a client.
///
/// # Arguments:
///
/// * `header` - The header of the response.
/// * `ceb_data` - The Clock Error Bound calculated from the Chrony tracking data.
fn build_response_now(header: Vec<u8>, ceb_data: &ClockErrorBound) -> Vec<u8> {
    let mut response: Vec<u8> = header;
    let (earliest, latest): (u64, u64) = clockbound_now(ceb_data.ceb);
    response.write_u64::<NetworkEndian>(earliest).unwrap();
    response.write_u64::<NetworkEndian>(latest).unwrap();
    response
}

/// Builds the response of a before or after request to send to a client.
///
/// # Arguments:
///
/// * `header` - The header of the response.
/// * `ceb_data` - The ClockErrorBound struct containing the CEB calculated from the Chrony tracking data.
/// * `time_epoch` - The timestamp in nanoseconds since the Unix Epoch to compare against.
fn build_response_before_after(
    header: Vec<u8>,
    ceb_data: &ClockErrorBound,
    time_epoch: u64,
) -> Vec<u8> {
    let mut response = header;
    let (earliest, latest) = clockbound_now(ceb_data.ceb);
    // response[1] holds the response type
    // 2 = Before
    // 3 = After
    if response[1] == 2 {
        response
            .write_u8(clockbound_before(earliest, time_epoch))
            .unwrap();
    } else if response[1] == 3 {
        response
            .write_u8(clockbound_after(latest, time_epoch))
            .unwrap();
    }
    response
}

/// Takes a Clock Error Bound and generates earliest and latest bounds based on the current system
/// time.
///
/// Calculation of bounds is computed with [System time - CEB, System time + CEB]
///
/// # Arguments:
///
/// * `ceb` - The Clock Error Bound calculated from the Chrony tracking data.
fn clockbound_now(ceb: f64) -> (u64, u64) {
    // For testing purposes we will use a mock function to retrieve our timestamp instead of getting
    // the current time.
    #[cfg(test)]
    let time_nanos: u64 = tests::mock_get_epoch_us();
    #[cfg(not(test))]
    let time_nanos: u64 = get_epoch_us();
    // Convert seconds to nanoseconds and round to the nearest nanosecond.
    let ceb_nanos: u64 = (ceb * 1000000000.0) as u64;
    return (time_nanos - ceb_nanos, time_nanos + ceb_nanos);
}

/// Takes the earliest bound calculated from a now request and compares it against a provided
/// timestamp represented as nanoseconds since the Unix epoch. If the timestamp provided is earlier
/// than the earliest bound then it returns true, otherwise false.
///
/// Earliest bound is computed as [System time - CEB]
///
/// # Arguments:
///
/// * `earliest` - The earliest bound calculated from a now request. Computed by [System time - CEB]
/// * `time_epoch` - The timestamp in nanoseconds since the Unix Epoch to compare against.
fn clockbound_before(earliest: u64, time_epoch: u64) -> u8 {
    return if time_epoch < earliest { 1 } else { 0 };
}

/// Takes the latest bound calculated from a now request and compares it against a provided
/// timestamp represented as nanoseconds since the Unix epoch. If the timestamp provided is later
/// than the latest bound then it returns true, otherwise false.
///
/// Latest bound is computed as [System time + CEB]
///
/// # Arguments:
///
/// * `latest` - The latest bound calculated from a now request. Computed by [System time + CEB]
/// * `time_epoch` - The timestamp in nanoseconds since the Unix Epoch to compare against.
fn clockbound_after(latest: u64, time_epoch: u64) -> u8 {
    return if time_epoch > latest { 1 } else { 0 };
}

/// Get the current system time in nanoseconds since the Unix epoch.
#[cfg(not(test))]
fn get_epoch_us() -> u64 {
    let now = Utc::now();
    now.timestamp_nanos() as u64
}

#[cfg(test)]
mod tests {
    use crate::ceb::ClockErrorBound;
    use crate::chrony_poller::LEAP_STATUS_UNSYNCHRONIZED;
    use crate::response::{
        build_response, build_response_header, clockbound_after, clockbound_before, clockbound_now,
        validate_request, RESPONSE_VERSION,
    };
    use crate::tracking::mock_tracking;
    use byteorder::NetworkEndian;
    use byteorder::{ReadBytesExt, WriteBytesExt};
    use chrono::{TimeZone, Utc};
    use std::convert::TryFrom;
    use std::io::Cursor;

    pub fn mock_get_epoch_us() -> u64 {
        // This equals 1000000000000000000 nanoseconds since the unix epoch
        let now = Utc.ymd(2001, 9, 9).and_hms_nano(1, 46, 40, 0);
        now.timestamp_nanos() as u64
    }

    #[test]
    fn test_build_response_now_successful() {
        let tracking = mock_tracking();

        let mut request: [u8; 12] = [0; 12];

        // Now request
        let request_type: u8 = 1;

        // Create a now request to test
        // Header
        // Version
        request[0] = RESPONSE_VERSION;
        // Command Type
        request[1] = request_type;

        let response = build_response(request, 4, tracking, false, 1.0);

        let mut rdr = Cursor::new(response);
        assert_eq!(RESPONSE_VERSION, rdr.read_u8().unwrap());
        assert_eq!(request_type, rdr.read_u8().unwrap());
        // Sync flag
        assert_eq!(0, rdr.read_u8().unwrap());
        // Reserved
        assert_eq!(0, rdr.read_u8().unwrap());
        // Get the CEB from mock tracking data
        let ceb = ClockErrorBound::from(tracking).ceb;
        let bounds = clockbound_now(ceb);
        // Earliest bound
        assert_eq!(bounds.0, rdr.read_u64::<NetworkEndian>().unwrap());
        // Latest bound
        assert_eq!(bounds.1, rdr.read_u64::<NetworkEndian>().unwrap());
    }

    #[test]
    fn test_build_response_before_true_successful() {
        let tracking = mock_tracking();

        let mut request: Vec<u8> = Vec::new();

        // Before request
        let request_type: u8 = 2;

        // Create a before request to test
        // Header
        // Version
        request.push(RESPONSE_VERSION);
        // Command Type
        request.push(request_type);
        // Reserved
        request.push(0);
        request.push(0);

        // Get the CEB from mock tracking data
        let ceb = ClockErrorBound::from(tracking).ceb;
        let bounds = clockbound_now(ceb);
        // 0 is the Unix Epoch
        let before_time = 0;

        request.write_u64::<NetworkEndian>(before_time).unwrap();

        let response = build_response(
            <[u8; 12]>::try_from(request).unwrap(),
            12,
            tracking,
            false,
            1.0,
        );

        let mut rdr = Cursor::new(response);
        assert_eq!(RESPONSE_VERSION, rdr.read_u8().unwrap());
        assert_eq!(request_type, rdr.read_u8().unwrap());
        // Sync flag
        assert_eq!(0, rdr.read_u8().unwrap());
        // Reserved
        assert_eq!(0, rdr.read_u8().unwrap());
        // Is 0 nanoseconds since epoch less than the mock current time of 1000000000000000000 nanoseconds since epoch?
        let before_flag = clockbound_before(bounds.0, before_time);
        // Before Flag. 0 < 1000000000000000000 should return 1
        assert_eq!(before_flag, rdr.read_u8().unwrap());
    }

    #[test]
    fn test_build_response_before_false_successful() {
        let tracking = mock_tracking();

        let mut request: Vec<u8> = Vec::new();

        // Before request
        let request_type: u8 = 2;

        // Create a before request to test
        // Header
        // Version
        request.push(RESPONSE_VERSION);
        // Command Type
        request.push(request_type);
        // Reserved
        request.push(0);
        request.push(0);

        // Get the CEB from mock tracking data
        let ceb = ClockErrorBound::from(tracking).ceb;
        let bounds = clockbound_now(ceb);
        // 1000000000000000001 is one nanosecond more than our mock data of 1000000000000000000
        let before_time = 1000000000000000001;

        request.write_u64::<NetworkEndian>(before_time).unwrap();

        let response = build_response(
            <[u8; 12]>::try_from(request).unwrap(),
            12,
            tracking,
            false,
            1.0,
        );

        let mut rdr = Cursor::new(response);
        assert_eq!(RESPONSE_VERSION, rdr.read_u8().unwrap());
        assert_eq!(request_type, rdr.read_u8().unwrap());
        // Sync flag
        assert_eq!(0, rdr.read_u8().unwrap());
        // Reserved
        assert_eq!(0, rdr.read_u8().unwrap());
        // Is 1000000000000000001 nanoseconds since epoch less than the mock current time of 1000000000000000000 nanoseconds since epoch?
        let before_flag = clockbound_before(bounds.0, before_time);
        // Before Flag. 1000000000000000001 < 1000000000000000000 should return 0
        assert_eq!(before_flag, rdr.read_u8().unwrap());
    }

    #[test]
    fn test_build_response_after_true_successful() {
        let tracking = mock_tracking();

        let mut request: Vec<u8> = Vec::new();

        // After request
        let request_type: u8 = 3;

        // Create a after request to test
        // Header
        // Version
        request.push(RESPONSE_VERSION);
        // Command Type
        request.push(request_type);
        // Reserved
        request.push(0);
        request.push(0);

        // Get the CEB from mock tracking data
        let ceb = ClockErrorBound::from(tracking).ceb;
        let bounds = clockbound_now(ceb);
        // 1000000000000000001 is one nanosecond more than our mock data of 1000000000000000000
        let after_time = 1000000000000000001;

        request.write_u64::<NetworkEndian>(after_time).unwrap();

        let response = build_response(
            <[u8; 12]>::try_from(request).unwrap(),
            12,
            tracking,
            false,
            1.0,
        );

        let mut rdr = Cursor::new(response);
        assert_eq!(RESPONSE_VERSION, rdr.read_u8().unwrap());
        assert_eq!(request_type, rdr.read_u8().unwrap());
        // Sync flag
        assert_eq!(0, rdr.read_u8().unwrap());
        // Reserved
        assert_eq!(0, rdr.read_u8().unwrap());
        // Is 1000000000000000001 nanoseconds since epoch greater than the mock current time of 1000000000000000000 nanoseconds since epoch?
        let after_flag = clockbound_after(bounds.1, after_time);
        // After Flag. 1000000000000000001 > 1000000000000000000 should return 1
        assert_eq!(after_flag, rdr.read_u8().unwrap());
    }

    #[test]
    fn test_build_response_after_false_successful() {
        let tracking = mock_tracking();

        let mut request: Vec<u8> = Vec::new();

        // After request
        let request_type: u8 = 3;

        // Create a after request to test
        // Header
        // Version
        request.push(RESPONSE_VERSION);
        // Command Type
        request.push(request_type);
        // Reserved
        request.push(0);
        request.push(0);

        // Get the CEB from mock tracking data
        let ceb = ClockErrorBound::from(tracking).ceb;
        let bounds = clockbound_now(ceb);
        // 0 is the Unix Epoch
        let after_time = 0;

        request.write_u64::<NetworkEndian>(after_time).unwrap();

        let response = build_response(
            <[u8; 12]>::try_from(request).unwrap(),
            12,
            tracking,
            false,
            1.0,
        );

        let mut rdr = Cursor::new(response);
        assert_eq!(RESPONSE_VERSION, rdr.read_u8().unwrap());
        assert_eq!(request_type, rdr.read_u8().unwrap());
        // Sync flag
        assert_eq!(0, rdr.read_u8().unwrap());
        // Reserved
        assert_eq!(0, rdr.read_u8().unwrap());
        // Is 0 nanoseconds since epoch greater than the mock current time of 1000000000000000000 nanoseconds since epoch?
        let after_flag = clockbound_after(bounds.1, after_time);
        // After Flag. 0 > 1000000000000000000 should return 0
        assert_eq!(after_flag, rdr.read_u8().unwrap());
    }

    #[test]
    fn test_build_response_header_error_successful() {
        // Any request other than 1, 2 or 3 should reply with an Error (0) response
        let request_type: u8 = 0;
        let header = build_response_header(request_type, 0);
        let mut rdr = Cursor::new(header);
        assert_eq!(RESPONSE_VERSION, rdr.read_u8().unwrap());
        assert_eq!(request_type, rdr.read_u8().unwrap());
        //sync flag
        assert_eq!(0, rdr.read_u8().unwrap());
        //reserved
        assert_eq!(0, rdr.read_u8().unwrap());

        // Test a non 0 value as well.
        let request_type: u8 = 8;
        let header = build_response_header(request_type, 0);
        let mut rdr = Cursor::new(header);
        assert_eq!(RESPONSE_VERSION, rdr.read_u8().unwrap());
        // Should respond with an Error (0) response
        assert_eq!(0, rdr.read_u8().unwrap());
        //sync flag
        assert_eq!(0, rdr.read_u8().unwrap());
        //reserved
        assert_eq!(0, rdr.read_u8().unwrap());
    }

    #[test]
    fn test_validate_request_valid() {
        // Valid Now request
        assert_eq!(validate_request(RESPONSE_VERSION, 1, 4), true);

        // Valid Before request
        assert_eq!(validate_request(RESPONSE_VERSION, 2, 12), true);

        // Valid After request
        assert_eq!(validate_request(RESPONSE_VERSION, 3, 12), true);
    }

    #[test]
    fn test_validate_request_invalid() {
        // Invalid request version
        assert_eq!(validate_request(0, 1, 4), false);

        // Invalid request type
        assert_eq!(validate_request(RESPONSE_VERSION, 0, 4), false);

        // Invalid Now request size
        assert_eq!(validate_request(RESPONSE_VERSION, 1, 12), false);

        // Invalid Before request size
        assert_eq!(validate_request(RESPONSE_VERSION, 2, 4), false);

        // Invalid After request size
        assert_eq!(validate_request(RESPONSE_VERSION, 3, 4), false);
    }

    #[test]
    fn test_build_response_sync_flag_false() {
        let mut tracking = mock_tracking();

        tracking.leap_status = LEAP_STATUS_UNSYNCHRONIZED;

        let mut request: [u8; 12] = [0; 12];

        // Now request
        let request_type: u8 = 1;

        // Create a now request to test
        // Header
        // Version
        request[0] = RESPONSE_VERSION;
        // Command Type
        request[1] = request_type;

        let response = build_response(request, 4, tracking, false, 1.0);

        let mut rdr = Cursor::new(response);
        assert_eq!(RESPONSE_VERSION, rdr.read_u8().unwrap());
        assert_eq!(request_type, rdr.read_u8().unwrap());
        // Sync flag should be false
        assert_eq!(1, rdr.read_u8().unwrap());
        // Reserved
        assert_eq!(0, rdr.read_u8().unwrap());
        // Should still build the body of the response with the sync flag as false
        // Get the CEB from mock tracking data
        let ceb = ClockErrorBound::from(tracking).ceb;
        let bounds = clockbound_now(ceb);
        // Earliest bound
        assert_eq!(bounds.0, rdr.read_u64::<NetworkEndian>().unwrap());
        // Latest bound
        assert_eq!(bounds.1, rdr.read_u64::<NetworkEndian>().unwrap());
    }
}
