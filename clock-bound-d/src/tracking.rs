// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
#[cfg(test)]
use chrony_candm::common::ChronyAddr;
use chrony_candm::common::ChronyFloat;
use chrony_candm::reply::Tracking;
#[cfg(not(test))]
use std::io;
#[cfg(not(test))]
use std::io::{Error, ErrorKind};
#[cfg(test)]
use std::net::IpAddr;
use std::time::SystemTime;
use crate::NANOSEC_IN_SEC;

/// Compute current root dispersion.
///
/// The root dispersion grows at the calculated error rate per second. Note that this bound may be
/// slightly inflated when compared with the one emitted by the local NTP daemon. The polling
/// of the local NTP daemon may occur slightly after its internal state has changed. In
/// particular:
///
/// - the result is inflated by the delay in getting the latest update at which the linear
///   growth is reset.
/// - the polled residual frequency is always slightly higher as the clock error correction
///   converges to the intended value.
///
/// The impact of the above is minimal, and can only make the daemon look slightly worse. This
/// more pessimistic bound is acceptable.
///
/// # Arguments
///
/// * `tracking` - The tracking information received from Chrony.
/// * `time` - The time to calculate the current root dispersion for
/// * `max_clock_error` - The assumed maximum frequency error that a system clock can gain between updates in ppm.
fn dispersion_at(tracking: Tracking, time: &SystemTime, max_clock_error: f64) -> f64 {
    let dur = time.duration_since(tracking.ref_time).unwrap();
    let dur = (dur.as_secs() as f64) + (dur.subsec_nanos() / NANOSEC_IN_SEC) as f64;
    // Calculate error rate using tracking information
    let error_rate =
        (max_clock_error + f64::from(tracking.skew_ppm) + f64::from(tracking.resid_freq_ppm))
            * 1e-6;
    f64::from(tracking.root_dispersion) + dur * error_rate
}

/// Update the root dispersion of the tracking data received from Chrony.
///
/// # Arguments
///
/// * `tracking_data` - The tracking information received from Chrony.
/// * `max_clock_error` - The assumed maximum frequency error that a system clock can gain between updates in ppm.
#[cfg(not(test))]
pub fn update_root_dispersion(
    tracking_data: Tracking,
    max_clock_error: f64,
) -> Result<Tracking, io::Error> {
    let mut tracking = tracking_data.clone();

    if tracking.ref_time > SystemTime::now() {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!(
                "Chrony's last update is in the future {:?}",
                tracking.ref_time
            ),
        ));
    };

    let now = SystemTime::now();
    tracking.root_dispersion = ChronyFloat::from(dispersion_at(tracking, &now, max_clock_error));
    Ok(tracking)
}

/// Create a mock tracking structure for testing
#[cfg(test)]
pub fn mock_tracking() -> Tracking {
    let d_chrony_float = ChronyFloat::from(f64::default());
    let d_chrony_addr = ChronyAddr::from(IpAddr::from([0; 4]));
    let d_u32 = u32::default();
    let d_u16 = u16::default();
    Tracking {
        ip_addr: d_chrony_addr,
        current_correction: ChronyFloat::from(0.000000001_f64),
        freq_ppm: d_chrony_float,
        last_offset: d_chrony_float,
        last_update_interval: d_chrony_float,
        leap_status: d_u16,
        ref_time: SystemTime::UNIX_EPOCH,
        ref_id: d_u32,
        resid_freq_ppm: d_chrony_float,
        rms_offset: d_chrony_float,
        root_delay: ChronyFloat::from(0.000000004_f64),
        root_dispersion: ChronyFloat::from(0.000000002_f64),
        skew_ppm: d_chrony_float,
        stratum: d_u16,
    }
}

#[cfg(test)]
mod tests {
    use crate::tracking::{dispersion_at, mock_tracking};
    use std::time::Duration;

    #[test]
    fn test_dispersion_at() {
        let tracking = mock_tracking();
        let max_clock_error: f64 = 1.0;
        let dur_secs: f64 = 5.0;
        // Validate the new root dispersion is correct for a 5 second duration
        let time = tracking.ref_time + Duration::from_secs(dur_secs as u64);
        let new_root_dispersion = dispersion_at(tracking, &time, max_clock_error);

        // Calculate error rate using tracking information
        let error_rate =
            (max_clock_error + f64::from(tracking.skew_ppm) + f64::from(tracking.resid_freq_ppm))
                * 1e-6;

        // Formula is root_dispersion + duration * error_rate
        let expected_root_dispersion = f64::from(tracking.root_dispersion) + dur_secs * error_rate;
        assert_eq!(new_root_dispersion, expected_root_dispersion);
    }
}
