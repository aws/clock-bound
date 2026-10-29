//! Select from multiple clock sources

use std::collections::HashMap;

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
    tenures: SourceTenures,
}

impl Selector {
    /// Constructor
    pub fn new(max_dispersion_growth: Skew) -> Self {
        Self {
            current: None,
            max_dispersion_growth,
            tenures: SourceTenures::new(),
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
            self.tenures
                .record_selection(source_info.clone(), clock_parameters.time);
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
                if current.source_info == source_info {
                    (current.selected_at, current.selected_at_clock_error_bound)
                } else {
                    self.tenures
                        .record_deselection(&current.source_info, clock_parameters.time);
                    (clock_parameters.time, clock_parameters.clock_error_bound)
                };
            self.tenures
                .record_selection(source_info.clone(), selected_at);
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
    ///
    /// The selected source's tenure ends. A disruption has no timestamp here, so the tenure is
    /// stamped with the time of the source's latest selected sample instead. Tenures are kept,
    /// so a source can start a new one afterwards or stay deselected.
    pub fn handle_disruption(&mut self) {
        if let Some(current) = self.current.take() {
            self.tenures
                .record_deselection(&current.source_info, current.clock_parameters.time);
        }
    }

    /// Get the current best clock parameters
    pub fn current(&self) -> Option<&SyncParameters> {
        self.current.as_ref()
    }

    /// Get the most recent selection tenure of a source, if it has ever been selected.
    pub fn tenure_of(&self, source_info: &SourceInfo) -> Option<Tenure> {
        self.tenures.get(source_info)
    }

    /// Forget the tenure of a source that no longer exists.
    ///
    /// The selection is unchanged: a removed source that is still selected stays selected until
    /// another source displaces it.
    pub fn forget_source(&mut self, source_info: &SourceInfo) {
        self.tenures.forget(source_info);
    }
}

/// A source's most recent selection tenure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tenure {
    /// Time at which the source began its most recent tenure.
    pub selected_at: Instant,
    /// Time at which the source was displaced by another source, or its selection was cleared
    /// by a disruption. `None` while it is selected.
    pub deselected_at: Option<Instant>,
}

/// The most recent selection [`Tenure`] of every source that has been selected and still exists.
#[derive(Debug, Clone)]
struct SourceTenures(HashMap<SourceInfo, Tenure>);

impl SourceTenures {
    fn new() -> Self {
        Self(HashMap::new())
    }
    /// Record that a source has been selected since `selected_at`.
    ///
    /// Recording a source that is already selected, with its unchanged `selected_at`, changes
    /// nothing.
    fn record_selection(&mut self, source_info: SourceInfo, selected_at: Instant) {
        self.0.insert(
            source_info,
            Tenure {
                selected_at,
                deselected_at: None,
            },
        );
    }

    /// End the tenure of a source displaced by another one. Does nothing for a source without
    /// a tenure.
    fn record_deselection(&mut self, source_info: &SourceInfo, deselected_at: Instant) {
        if let Some(tenure) = self.0.get_mut(source_info) {
            tenure.deselected_at = Some(deselected_at);
        }
    }

    /// Drop a source's tenure.
    fn forget(&mut self, source_info: &SourceInfo) {
        self.0.remove(source_info);
    }

    /// A source's tenure, if it has one.
    fn get(&self, source_info: &SourceInfo) -> Option<Tenure> {
        self.0.get(source_info).copied()
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
            tenures: SourceTenures::new(),
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

    fn amazon_time_sync(addr: &str) -> SourceInfo {
        SourceInfo::AmazonTimeSync(addr.parse().unwrap())
    }

    #[test]
    fn fresh_selection_sets_tenure_fields() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let params = params_at(1_000, 10_000);
        let result = selector
            .update(&params, amazon_time_sync("169.254.169.123:123"))
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
            .update(&first, amazon_time_sync("169.254.169.123:123"))
            .unwrap();

        // The same source wins again, later in time and with a different (better)
        // clock error bound; the tenure start values are preserved.
        let second = params_at(5_000, 5_000);
        let result = selector
            .update(&second, amazon_time_sync("169.254.169.123:123"))
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
            .update(&first, amazon_time_sync("169.254.169.123:123"))
            .unwrap();

        // A different source (different variant + address) takes over; the
        // tenure restarts at the new winning update's values.
        let second = params_at(5_000, 5_000);
        let result = selector
            .update(
                &second,
                SourceInfo::NtpSource("169.254.169.101:123".parse().unwrap()),
            )
            .unwrap();
        assert_eq!(result.selected_at, second.time);
        assert_eq!(
            result.selected_at_clock_error_bound,
            second.clock_error_bound
        );
    }

    #[test]
    fn post_disruption_selection_gets_fresh_tenure_fields() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let first = params_at(1_000, 10_000);
        selector
            .update(&first, amazon_time_sync("169.254.169.123:123"))
            .unwrap();

        selector.handle_disruption();
        assert!(selector.current().is_none());

        // Even though it is the same source, selection state was cleared, so the
        // first post-disruption selection starts a fresh tenure.
        let second = params_at(9_000, 20_000);
        let result = selector
            .update(&second, amazon_time_sync("169.254.169.123:123"))
            .unwrap();
        assert_eq!(result.selected_at, second.time);
        assert_eq!(
            result.selected_at_clock_error_bound,
            second.clock_error_bound
        );
    }

    // -------------------------------------------------------------------------
    // Per-source tenure map (`tenure_of`)
    // -------------------------------------------------------------------------

    fn ntp_source(addr: &str) -> SourceInfo {
        SourceInfo::NtpSource(addr.parse().unwrap())
    }

    #[test]
    fn first_selection_starts_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let source = ntp_source("192.0.2.1:123");
        let params = params_at(1_000, 10_000);
        selector.update(&params, source.clone()).unwrap();

        assert_eq!(
            selector.tenure_of(&source),
            Some(Tenure {
                selected_at: params.time,
                deselected_at: None,
            })
        );
    }

    #[test]
    fn never_selected_source_has_no_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let selected = ntp_source("192.0.2.1:123");
        let rejected = ntp_source("192.0.2.2:123");
        selector
            .update(&params_at(1_000, 5_000), selected.clone())
            .unwrap();

        // A worse candidate keeps the current selection and gains no tenure.
        assert!(
            selector
                .update(&params_at(2_000, 10_000), rejected.clone())
                .is_none()
        );
        assert_eq!(selector.tenure_of(&rejected), None);
        assert_eq!(selector.tenure_of(&selected).unwrap().deselected_at, None);
    }

    #[test]
    fn handover_ends_displaced_tenure_and_starts_winner_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let first_source = ntp_source("192.0.2.1:123");
        let second_source = ntp_source("192.0.2.2:123");
        let first = params_at(1_000, 10_000);
        let second = params_at(5_000, 5_000);
        selector.update(&first, first_source.clone()).unwrap();
        selector.update(&second, second_source.clone()).unwrap();

        assert_eq!(
            selector.tenure_of(&first_source),
            Some(Tenure {
                selected_at: first.time,
                deselected_at: Some(second.time),
            })
        );
        assert_eq!(
            selector.tenure_of(&second_source),
            Some(Tenure {
                selected_at: second.time,
                deselected_at: None,
            })
        );
    }

    #[test]
    fn same_source_rewinning_keeps_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let source = ntp_source("192.0.2.1:123");
        let first = params_at(1_000, 10_000);
        selector.update(&first, source.clone()).unwrap();
        selector
            .update(&params_at(5_000, 5_000), source.clone())
            .unwrap();

        assert_eq!(
            selector.tenure_of(&source),
            Some(Tenure {
                selected_at: first.time,
                deselected_at: None,
            })
        );
    }

    #[test]
    fn disruption_ends_the_selected_sources_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let source = ntp_source("192.0.2.1:123");
        let first = params_at(1_000, 10_000);
        let latest = params_at(5_000, 5_000);
        selector.update(&first, source.clone()).unwrap();
        selector.update(&latest, source.clone()).unwrap();

        selector.handle_disruption();

        // The disruption has no timestamp, so the tenure ends at the latest selected sample.
        assert_eq!(
            selector.tenure_of(&source),
            Some(Tenure {
                selected_at: first.time,
                deselected_at: Some(latest.time),
            })
        );
    }

    #[test]
    fn source_left_behind_by_disruption_keeps_its_ended_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let left_behind = ntp_source("192.0.2.1:123");
        let winner = ntp_source("192.0.2.2:123");
        let first = params_at(1_000, 10_000);
        selector.update(&first, left_behind.clone()).unwrap();
        selector.handle_disruption();

        // A different source wins after the disruption; the first stays deselected.
        let second = params_at(9_000, 20_000);
        selector.update(&second, winner.clone()).unwrap();

        assert_eq!(
            selector.tenure_of(&left_behind),
            Some(Tenure {
                selected_at: first.time,
                deselected_at: Some(first.time),
            })
        );
        assert_eq!(
            selector.tenure_of(&winner),
            Some(Tenure {
                selected_at: second.time,
                deselected_at: None,
            })
        );
    }

    #[test]
    fn reselection_after_disruption_starts_fresh_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let source = ntp_source("192.0.2.1:123");
        selector
            .update(&params_at(1_000, 10_000), source.clone())
            .unwrap();
        selector.handle_disruption();
        let after = params_at(9_000, 20_000);
        selector.update(&after, source.clone()).unwrap();

        assert_eq!(
            selector.tenure_of(&source),
            Some(Tenure {
                selected_at: after.time,
                deselected_at: None,
            })
        );
    }

    #[test]
    fn forgetting_a_displaced_source_drops_only_its_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let displaced = ntp_source("192.0.2.1:123");
        let selected = ntp_source("192.0.2.2:123");
        selector
            .update(&params_at(1_000, 10_000), displaced.clone())
            .unwrap();
        selector
            .update(&params_at(5_000, 5_000), selected.clone())
            .unwrap();

        selector.forget_source(&displaced);

        assert_eq!(selector.tenure_of(&displaced), None);
        assert!(selector.tenure_of(&selected).is_some());
    }

    #[test]
    fn forgetting_the_selected_source_leaves_nothing_behind() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let removed = ntp_source("192.0.2.1:123");
        let winner = ntp_source("192.0.2.2:123");
        selector
            .update(&params_at(1_000, 10_000), removed.clone())
            .unwrap();

        selector.forget_source(&removed);

        // Removal drops the tenure but leaves the selection alone.
        assert_eq!(selector.tenure_of(&removed), None);
        assert_eq!(selector.current().unwrap().source_info, removed);

        // Displacing the removed source has no tenure to end, and records none for it.
        let second = params_at(5_000, 5_000);
        selector.update(&second, winner.clone()).unwrap();
        assert_eq!(selector.tenure_of(&removed), None);
        assert_eq!(
            selector.tenure_of(&winner),
            Some(Tenure {
                selected_at: second.time,
                deselected_at: None,
            })
        );
    }

    #[test]
    fn removed_selected_source_winning_again_records_its_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let source = ntp_source("192.0.2.1:123");
        let first = params_at(1_000, 10_000);
        selector.update(&first, source.clone()).unwrap();

        // The source is removed and re-added while still selected, then wins again. It is the
        // same source, so its tenure carries on from the original `selected_at`.
        selector.forget_source(&source);
        selector
            .update(&params_at(5_000, 5_000), source.clone())
            .unwrap();

        assert_eq!(
            selector.tenure_of(&source),
            Some(Tenure {
                selected_at: first.time,
                deselected_at: None,
            })
        );
    }

    #[test]
    fn forgetting_an_unknown_source_is_a_no_op() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        selector.forget_source(&ntp_source("192.0.2.1:123"));
        assert!(selector.current().is_none());
    }

    #[test]
    fn reselection_after_deselection_starts_fresh_tenure() {
        let mut selector = test_selector(Skew::from_ppm(0.0));
        let first_source = ntp_source("192.0.2.1:123");
        let second_source = ntp_source("192.0.2.2:123");
        selector
            .update(&params_at(1_000, 10_000), first_source.clone())
            .unwrap();
        selector
            .update(&params_at(5_000, 5_000), second_source.clone())
            .unwrap();
        let third = params_at(9_000, 1_000);
        selector.update(&third, first_source.clone()).unwrap();

        assert_eq!(
            selector.tenure_of(&first_source),
            Some(Tenure {
                selected_at: third.time,
                deselected_at: None,
            })
        );
        assert_eq!(
            selector.tenure_of(&second_source).unwrap().deselected_at,
            Some(third.time)
        );
    }
}
