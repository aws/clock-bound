// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
use chrony_candm::reply::Tracking;
use crate::{PhcErrorBound, NANOSEC_IN_SEC};

/// A struct containing the Clock Error Bound. The Clock Error Bound is the bound of error that is
/// accumulated for a NTP packet.
///
/// Clock Error Bound is calculated with the formula:
///
/// |System time offset| + Root dispersion + (Root delay / 2)
///
/// Where:
///
/// System time offset - Difference between chrony's estimate of the "true time" from it's root
/// reference and the system's clock.
///
/// Root dispersion - Sum of dispersion across each strata.
///
/// Root delay - Sum of network latency accumulated across each strata.
#[derive(Clone, Debug)]
pub struct ClockErrorBound {
    pub ceb: f64,
}

impl ClockErrorBound {
    /// Calculate the Clock Error Bound using the Tracking information from Chrony and the
    /// PhcErrorBound from sysfs if Chrony is syncing to it.
    pub fn from(packet: Tracking, phc_error_bound: PhcErrorBound) -> ClockErrorBound {
        ClockErrorBound {
            ceb: get_clock_error_bound(
                f64::from(packet.current_correction),
                f64::from(packet.root_dispersion),
                f64::from(packet.root_delay),
                phc_error_bound,
            ),
        }
    }
}

/// Get the Clock Error Bound.
///
/// Clock Error Bound is calculated with the formula:
/// |System time offset| + Root dispersion + (Root delay / 2)
///
/// # Arguments
/// * `system_time_offset` - Difference between chrony's estimate of the "true time" from it's root
/// reference and the system's clock.
/// * `root_dispersion` - Sum of dispersion across each strata.
/// * `root_delay` - Sum of network latency accumulated across each strata.
pub fn get_clock_error_bound(
    system_time_offset: f64,
    root_dispersion: f64,
    root_delay: f64,
    phc_error_bound: f64,
) -> f64 {
    round_f64_nanos(system_time_offset.abs() + root_dispersion + (root_delay / 2_f64) + (phc_error_bound / NANOSEC_IN_SEC as f64))
}

/// Round a f64 to nanosecond precision.
///
/// A ChronyFloat as defined by Chrony can introduce some loss when converting from a f64.
/// However, the loss is in a value of precision that is not needed by ClockBound. Since ClockBound
/// provides bounds in the nanosecond accuracy this extra loss in precision can be ignored by
/// rounding to the nearest nanosecond.
///
/// # Arguments
/// * `value` - A f64 value to round to the nearest nanosecond.
pub fn round_f64_nanos(value: f64) -> f64 {
    (value * 1000000000.0).round() / 1000000000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_f64_nanos_successful() {
        let value = round_f64_nanos(0.0000000055_f64);
        assert_eq!(value, 0.000000006);
    }

    #[test]
    fn get_clock_error_bound_successful() {
        let ceb = get_clock_error_bound(0.0002_f64, 0.0001_f64, 0.0004_f64, 30_000_f64);
        assert_eq!(ceb, 0.00053_f64);
    }
}
