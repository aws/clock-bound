//! Calculated clock parameters
//!
//! The output of the [`ClockSyncAlgorithm`](super::clock_sync_algorithm)

use crate::daemon::time::{
    Duration, Instant, TscCount,
    tsc::{Period, Skew},
};

/// Clock parameters
///
/// These values are calculated by the [`ClockSyncAlgorithm`](super::clock_sync_algorithm)
/// and used by the [`ClockState`](super::clock_state)
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClockParameters {
    /// The tsc values that these account for
    pub tsc_count: TscCount,
    /// The time at `tsc_count`
    pub time: Instant,
    /// The clock error bound of `time` at `tsc_count`
    pub clock_error_bound: Duration,
    /// The period of the TSC clock at `tsc_count`
    pub period: Period,
    /// The max error of the `period` at `tsc_count`
    pub period_max_error: Period,
    /// The `CLOCK_MONOTONIC_COARSE` time just before these parameters are calculated.
    /// FIXME: remove when ClockBound 2.0 clients are not supported anymore.
    #[serde(skip)]
    pub as_of_monotonic: Instant,
}

impl ClockParameters {
    /// Compare another `ClockParameter`
    ///
    /// Returns true if `self` is more accurate than `rhs` clock parameters
    ///
    /// # Parameters
    /// - `rhs`: the other `ClockParameters` to compare against
    /// - `max_dispersion`: The maximum potential CPU drift
    pub fn more_accurate_than(&self, rhs: &ClockParameters, max_dispersion: Skew) -> bool {
        let mut self_ceb = self.clock_error_bound;
        let mut rhs_ceb = rhs.clock_error_bound;

        // Apply max dispersion aging to older sample
        let tsc_age = (rhs.tsc_count - self.tsc_count).abs();
        let accumulated_dispersion =
            (tsc_age * self.period).as_seconds_f64() * max_dispersion.get();
        let accumulated_dispersion = Duration::from_seconds_f64(accumulated_dispersion);
        if self.tsc_count < rhs.tsc_count {
            self_ceb += accumulated_dispersion;
        } else {
            rhs_ceb += accumulated_dispersion;
        }

        rhs_ceb > self_ceb
    }
}

/// Information on the selected clock
///
/// This struct is stored in the [`ClockSyncAlgorithm`](super::clock_sync_algorithm) as the
/// final output product of the algorithm.
///
/// Includes [`ClockParameters`] as well as information about the
/// selected clock
///
/// TODO: Include the name of the source, as well.
pub struct SelectedClockInfo {
    /// Calculated clock parameters from the [`ClockSyncAlgorithm`](super::clock_sync_algorithm)
    pub clock_parameters: ClockParameters,
    /// Stratum of the selected clock
    ///
    /// None if reading from a non-NTP device
    pub stratum: Option<u8>, // TODO: use the enum in another PR
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::daemon::event::{self, Stratum, TscRtt};
    use rstest::rstest;

    #[rstest]
    #[case::same_events_zero_skew(
        ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1) + Duration::from_nanos(500), // unused
        },
        // Second event (identical)
        event::Ntp::builder()
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(1),
                root_delay: Duration::from_micros(10),
                root_dispersion: Duration::from_micros(5),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Skew::from_ppm(0.0),
        false  // First event should be chosen when equal
    )]
    #[case::different_rtt_zero_skew(
        // First event with better RTT
        ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1) + Duration::from_nanos(500), // unused
        },
        // Second event with worse RTT
        event::Ntp::builder()
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_002_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(1),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(5),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Skew::from_ppm(15.0),
        false,
    )]
    #[case::time_difference_with_skew(
        // First event (older)
        ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1) + Duration::from_nanos(500), // unused
        },
        // Second event (newer, 1 second later)
        event::Ntp::builder()
            .tsc_pre(TscCount::new(2_000_000_000))
            .tsc_post(TscCount::new(2_000_001_000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1) + Duration::from_secs(1),
                server_send_time: Instant::from_days(1) + Duration::from_secs(1) + Duration::from_micros(1),
                root_delay: Duration::from_micros(10),
                root_dispersion: Duration::from_micros(5),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Skew::from_ppm(25.0),
        true
    )]
    #[case::different_period(
        // First event
        ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1) + Duration::from_nanos(500), // unused
        },
        // Second event
        event::Ntp::builder()
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_003_300))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(1),
                root_delay: Duration::from_micros(10),
                root_dispersion: Duration::from_micros(5),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(3.3e-9),
        Skew::from_ppm(10.0),
        false,
    )]
    #[case::first_better_despite_age(
        // First event
        ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1) + Duration::from_nanos(500), // unused
        },
        // Second event
        event::Ntp::builder()
            .tsc_pre(TscCount::new(5_000_000_000))
            .tsc_post(TscCount::new(5_000_003_300))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(1),
                root_delay: Duration::from_micros(10),
                root_dispersion: Duration::from_micros(50), // CEB of second degraded
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(0.303e-9),
        Skew::from_ppm(10.0),
        false
    )]
    fn compare_clock_error_bound(
        #[case] first: ClockParameters,
        #[case] second: event::Ntp,
        #[case] period: Period,
        #[case] max_dispersion: Skew,
        #[case] expected: bool,
    ) {
        let val = ClockParameters {
            tsc_count: second.tsc_midpoint(),
            time: second
                .data()
                .server_recv_time
                .midpoint(second.data().server_send_time),
            clock_error_bound: second.calculate_clock_error_bound(period),
            period,
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1) + Duration::from_nanos(500), // unused
        };
        let result = val.more_accurate_than(&first, max_dispersion);
        assert_eq!(result, expected);
    }

    #[rstest]
    #[case::high_skew_old_vs_new(
        // First event (old)
        ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1) + Duration::from_nanos(500), // unused
        },
        // Second event (new, 10 seconds later)
        event::Ntp::builder()
            .tsc_pre(TscCount::new(11_000_000_000))
            .tsc_post(TscCount::new(11_000_001_500))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1) + Duration::from_secs(10),
                server_send_time: Instant::from_days(1) + Duration::from_secs(10) + Duration::from_micros(1),
                root_delay: Duration::from_micros(12),
                root_dispersion: Duration::from_micros(6),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Skew::from_ppm(25.0),
        true  // Despite slightly worse metrics, newer sample should win due to age
    )]
    fn compare_clock_error_bound_with_aging(
        #[case] first: ClockParameters,
        #[case] second: event::Ntp,
        #[case] period: Period,
        #[case] max_dispersion: Skew,
        #[case] expected: bool,
    ) {
        let val = ClockParameters {
            tsc_count: second.tsc_midpoint(),
            time: second
                .data()
                .server_recv_time
                .midpoint(second.data().server_send_time),
            clock_error_bound: second.calculate_clock_error_bound(period),
            period,
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1) + Duration::from_nanos(500), // unused
        };
        let result = val.more_accurate_than(&first, max_dispersion);
        assert_eq!(result, expected);
    }
}
