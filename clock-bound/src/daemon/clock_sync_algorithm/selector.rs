//! Select from multiple clock sources

use std::net::SocketAddr;

use crate::daemon::{clock_parameters::ClockParameters, event::Stratum, time::tsc::Skew};

/// Select from multiple clock sources
///
/// Takes in [`ClockParameters`] values from multiple clock sources and
/// decides if it's a more accurate than the current best.
///
/// The current methodology is to purely compare the `ClockErrorBound` of inputs against the current best and update
/// if the clock error bound is lower. This does take into account dispersion growth via the `max_dispersion_growth`
/// parameter.
///
/// When picking the TSC Period to use, the current `ClockParameters` value is used
#[derive(Debug, Clone)]
pub struct Selector {
    current: Option<SourceParams>,
    max_dispersion_growth: Skew,
}

impl Selector {
    /// Constructor
    pub fn new(max_dispersion_growth: Skew) -> Self {
        Self {
            current: None,
            max_dispersion_growth,
        }
    }

    /// Compare an input `ClockParameters` against the current best
    ///
    /// Returns `Some` if the new value is more accurate than the current best. None otherwise.
    pub fn update(
        &mut self,
        clock_parameters: &ClockParameters,
        source_info: SourceInfo,
    ) -> Option<&ClockParameters> {
        let Some(current) = &self.current else {
            self.current = Some(SourceParams {
                clock_parameters: clock_parameters.clone(),
                source_info,
            });
            return self.current.as_ref().map(|sp| &sp.clock_parameters);
        };

        if current
            .clock_parameters
            .more_accurate_than(clock_parameters, self.max_dispersion_growth)
        {
            None
        } else {
            self.current = Some(SourceParams {
                clock_parameters: clock_parameters.clone(),
                source_info,
            });
            self.current.as_ref().map(|sp| &sp.clock_parameters)
        }
    }

    /// Clear inner state during a disruption event
    pub fn handle_disruption(&mut self) {
        let Self {
            current,
            max_dispersion_growth: _,
        } = self;
        *current = None;
    }

    /// Get the current best clock parameters
    pub fn current(&self) -> Option<&SourceParams> {
        self.current.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SourceParams {
    pub clock_parameters: ClockParameters,
    pub source_info: SourceInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceInfo {
    /// Amazon Time Sync
    AmazonTimeSync(SocketAddr, Stratum),
    /// NTP Source,
    NtpSource(SocketAddr, Stratum),
    /// PHC
    Phc,
}

#[cfg(test)]
mod tests {
    use crate::daemon::{
        event::{self, TscRtt},
        time::{Duration, Instant, TscCount, tsc::Period},
    };

    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::same_events_zero_skew(
        ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1), // unused
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
        true,
    )]
    #[case::different_rtt_zero_skew(
        // First event with better RTT
        ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1), // unused
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
            as_of_monotonic: Instant::from_days(1), // unused
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
            as_of_monotonic: Instant::from_days(1), // unused
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
            as_of_monotonic: Instant::from_days(1), // unused
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
    fn update(
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
            as_of_monotonic: Instant::from_days(1),        // unused
        };
        let mut selector = Selector {
            current: Some(SourceParams {
                clock_parameters: first,
                source_info: SourceInfo::Phc,
            }),
            max_dispersion_growth: max_dispersion,
        };
        let result = selector.update(&val, SourceInfo::Phc).is_some();
        assert_eq!(result, expected);
    }

    #[test]
    fn first_update_sets_current() {
        let clock_parameters = ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1), // unused
        };
        let mut selector = Selector::new(Skew::from_ppm(0.0));
        assert!(selector.current().is_none());
        let result = selector.update(&clock_parameters, SourceInfo::Phc).unwrap();
        assert_eq!(result, &clock_parameters);
        assert_eq!(selector.current().unwrap().source_info, SourceInfo::Phc);
    }

    #[test]
    fn handle_disruption() {
        let clock_parameters = ClockParameters {
            tsc_count: TscCount::new(1_000_000_500),
            time: Instant::from_days(1) + Duration::from_nanos(500),
            clock_error_bound: Duration::from_nanos(10_500),
            period: Period::from_seconds(1e-9), // unused
            period_max_error: Period::from_seconds(1e-11), // unused
            as_of_monotonic: Instant::from_days(1), // unused
        };
        let skew = Skew::from_ppm(1.0);
        let mut selector = Selector::new(skew);
        selector.update(&clock_parameters, SourceInfo::Phc).unwrap();
        selector.handle_disruption();
        assert!(selector.current().is_none());
        assert_eq!(selector.max_dispersion_growth, skew)
    }
}
