//! Select from multiple clock sources

use crate::daemon::{
    clock_parameters::ClockParameters,
    time::{Duration, Instant, tsc::Skew},
};

use super::source::SourceInfo;

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
    current: Option<SyncParameters>,
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
    ) -> Option<&SyncParameters> {
        let Some(current) = &self.current else {
            self.current = Some(SyncParameters {
                clock_parameters: clock_parameters.clone(),
                source_info,
                selected_at: clock_parameters.time,
                selected_at_clock_error_bound: clock_parameters.clock_error_bound,
            });
            return self.current.as_ref();
        };

        if current
            .clock_parameters
            .more_accurate_than(clock_parameters, self.max_dispersion_growth)
        {
            None
        } else {
            let (selected_at, selected_clock_error_bound) =
                // If the current source is the same as the newly selected one,
                // then we'll maintain the same `selected_at` and `selected_at_clock_error_bound`
                // values.
                if current.source_info.same_source(&source_info) {
                    (current.selected_at, current.selected_at_clock_error_bound)
                } else {
                    (clock_parameters.time, clock_parameters.clock_error_bound)
                };
            self.current = Some(SyncParameters {
                clock_parameters: clock_parameters.clone(),
                source_info,
                selected_at,
                selected_at_clock_error_bound: selected_clock_error_bound,
            });
            self.current.as_ref()
        }
    }

    /// Clear inner state during a disruption event.
    pub fn handle_disruption(&mut self) {
        self.current = None;
    }

    /// Get the current best clock parameters
    pub fn current(&self) -> Option<&SyncParameters> {
        self.current.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SyncParameters {
    pub clock_parameters: ClockParameters,
    pub source_info: SourceInfo,
    /// Timestamp at which the selected source began its current
    /// selection tenure.
    pub selected_at: Instant,
    /// The source's clock error bound at the start of its current selection
    /// tenure (i.e. the clock error bound reported alongside [`selected_at`](Self::selected_at)).
    pub selected_at_clock_error_bound: Duration,
}

#[cfg(test)]
mod tests {
    use crate::daemon::{
        event::{self, Stratum, TscRtt},
        time::{Duration, Instant, TscCount, tsc::Period},
    };

    use super::*;
    use rstest::rstest;

    fn test_selector(max_dispersion: Skew) -> Selector {
        Selector::new(max_dispersion)
    }

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
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_001_000))
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
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_002_000))
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
            .counter_pre(TscCount::new(2_000_000_000))
            .counter_post(TscCount::new(2_000_001_000))
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
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_003_300))
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
            .counter_pre(TscCount::new(5_000_000_000))
            .counter_post(TscCount::new(5_000_003_300))
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
            current: Some(SyncParameters {
                clock_parameters: first,
                source_info: SourceInfo::Phc("/dev/ptp0".into()),
                selected_at: Instant::from_days(1),
                selected_at_clock_error_bound: Duration::from_nanos(10_500),
            }),
            max_dispersion_growth: max_dispersion,
        };
        let result = selector
            .update(&val, SourceInfo::Phc("/dev/ptp0".into()))
            .is_some();
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
        let mut selector = test_selector(Skew::from_ppm(0.0));
        assert!(selector.current().is_none());
        let result = selector
            .update(&clock_parameters, SourceInfo::Phc("/dev/ptp0".into()))
            .unwrap();
        assert_eq!(&result.clock_parameters, &clock_parameters);
        assert_eq!(result.source_info, SourceInfo::Phc("/dev/ptp0".into()));
        // A fresh selection sets both tenure fields from the winning update.
        assert_eq!(result.selected_at, clock_parameters.time);
        assert_eq!(
            result.selected_at_clock_error_bound,
            clock_parameters.clock_error_bound
        );
        assert_eq!(
            selector.current().unwrap().source_info,
            SourceInfo::Phc("/dev/ptp0".into())
        );
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
        let mut selector = test_selector(skew);
        selector
            .update(&clock_parameters, SourceInfo::Phc("/dev/ptp0".into()))
            .unwrap();
        selector.handle_disruption();
        assert!(selector.current().is_none());
        assert_eq!(selector.max_dispersion_growth, skew);
    }

    // -------------------------------------------------------------------------
    // Selection tenure tracking (`selected_at` / `selected_clock_error_bound`)
    // -------------------------------------------------------------------------

    fn params_at(time_ns: i64, clock_error_bound_ns: i64) -> ClockParameters {
        ClockParameters {
            tsc_count: TscCount::new(1_000_000),
            time: Instant::from_nanos(time_ns),
            clock_error_bound: Duration::from_nanos(clock_error_bound_ns),
            period: Period::from_seconds(1e-9),
            period_max_error: Period::from_seconds(1e-11),
            as_of_monotonic: Instant::new(0),
        }
    }

    fn amazon_time_sync(addr: &str, stratum: Stratum) -> SourceInfo {
        SourceInfo::AmazonTimeSync(addr.parse().unwrap(), stratum)
    }

    #[test]
    fn fresh_selection_sets_tenure_fields() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let params = params_at(1_000, 10_000);
        let result = selector
            .update(
                &params,
                amazon_time_sync("169.254.169.123:123", Stratum::ONE),
            )
            .unwrap();
        assert_eq!(result.selected_at, params.time);
        assert_eq!(
            result.selected_at_clock_error_bound,
            params.clock_error_bound
        );
    }

    #[test]
    fn same_source_rewinning_carries_tenure_fields_forward() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let first = params_at(1_000, 10_000);
        selector
            .update(
                &first,
                amazon_time_sync("169.254.169.123:123", Stratum::ONE),
            )
            .unwrap();

        // The same source wins again, later in time and with a different (better)
        // clock error bound; the tenure start values are preserved.
        let second = params_at(5_000, 5_000);
        let result = selector
            .update(
                &second,
                amazon_time_sync("169.254.169.123:123", Stratum::ONE),
            )
            .unwrap();
        assert_eq!(result.selected_at, first.time);
        assert_eq!(
            result.selected_at_clock_error_bound,
            first.clock_error_bound
        );
    }

    #[test]
    fn different_source_resets_tenure_fields() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let first = params_at(1_000, 10_000);
        selector
            .update(
                &first,
                amazon_time_sync("169.254.169.123:123", Stratum::ONE),
            )
            .unwrap();

        // A different source (different variant + address) takes over; the
        // tenure restarts at the new winning update's values.
        let second = params_at(5_000, 5_000);
        let result = selector
            .update(
                &second,
                SourceInfo::NtpSource("169.254.169.101:123".parse().unwrap(), Stratum::ONE),
            )
            .unwrap();
        assert_eq!(result.selected_at, second.time);
        assert_eq!(
            result.selected_at_clock_error_bound,
            second.clock_error_bound
        );
    }

    #[test]
    fn same_address_stratum_change_does_not_reset_tenure_fields() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let first = params_at(1_000, 10_000);
        selector
            .update(
                &first,
                amazon_time_sync("169.254.169.123:123", Stratum::ONE),
            )
            .unwrap();

        // Same address, different stratum: still the same source, so the tenure
        // must NOT reset even though the clock error bound also changed.
        let second = params_at(5_000, 5_000);
        let result = selector
            .update(
                &second,
                amazon_time_sync("169.254.169.123:123", Stratum::TWO),
            )
            .unwrap();
        assert_eq!(result.selected_at, first.time);
        assert_eq!(
            result.selected_at_clock_error_bound,
            first.clock_error_bound
        );
    }

    #[test]
    fn post_disruption_selection_gets_fresh_tenure_fields() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let first = params_at(1_000, 10_000);
        selector
            .update(
                &first,
                amazon_time_sync("169.254.169.123:123", Stratum::ONE),
            )
            .unwrap();

        selector.handle_disruption();
        assert!(selector.current().is_none());

        // Even though it is the same source, selection state was cleared, so the
        // first post-disruption selection starts a fresh tenure.
        let second = params_at(9_000, 20_000);
        let result = selector
            .update(
                &second,
                amazon_time_sync("169.254.169.123:123", Stratum::ONE),
            )
            .unwrap();
        assert_eq!(result.selected_at, second.time);
        assert_eq!(
            result.selected_at_clock_error_bound,
            second.clock_error_bound
        );
    }
}
