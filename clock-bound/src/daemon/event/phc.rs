//! PHC Time synchronization events

use crate::daemon::{
    clock_sync_algorithm::ff::{LocalPeriodAndError, UncorrectedClock},
    time::{Duration, Instant, TscCount, tsc::Period},
};

use super::TscRtt;

/// Contains the PHC and time stamp counter samples to be used by the synchronization algorithm
///
/// `counter_post` must be greater than `counter_pre`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phc {
    /// TSC value before reading reference clock
    counter_pre: TscCount,
    /// TSC value after reading reference clock
    counter_post: TscCount,
    /// PHC reference clock info
    data: PhcData,
    #[cfg(not(test))]
    system_clock: Option<super::SystemClockMeasurement>,
}

#[bon::bon]
impl Phc {
    /// Construct a [`Phc`]
    ///
    /// Returns `None` if `counter_post <= counter_pre`
    #[builder]
    pub fn new(
        counter_pre: TscCount,
        counter_post: TscCount,
        data: PhcData,
        #[cfg(not(test))] system_clock: Option<super::SystemClockMeasurement>,
    ) -> Option<Self> {
        if counter_post <= counter_pre {
            return None;
        }

        Some(Self {
            counter_pre,
            counter_post,
            data,
            #[cfg(not(test))]
            system_clock,
        })
    }
}

impl Phc {
    /// `counter_pre` getter
    pub fn counter_pre(&self) -> TscCount {
        self.counter_pre
    }

    /// `counter_post` getter
    pub fn counter_post(&self) -> TscCount {
        self.counter_post
    }

    /// phc data getter
    pub fn data(&self) -> &PhcData {
        &self.data
    }

    /// system time getter
    #[cfg(not(test))]
    pub fn system_clock(&self) -> Option<&super::SystemClockMeasurement> {
        self.system_clock.as_ref()
    }

    /// Calculate a period by using 2 PHC events using tsc midpoints
    ///
    /// PHC reads are characterized in ClockBound with each read having
    /// - `counter_pre`: The TSC reading before reading the PHC device
    /// - `time`: The PHC time reading
    /// - `counter_post`: The TSC reading after reading the PHC device
    ///
    /// # Panics
    /// Panics if the events are the same (specifically if the `counter_post` values are equal)
    pub fn calculate_period(&self, other: &Self) -> Period {
        (self.data().time - other.data().time) / (self.tsc_midpoint() - other.tsc_midpoint())
    }

    /// Calculate the period along with the error in the period estimation
    ///
    /// When calculating the period, the error in the measurement has a direct relationship with
    /// the reference clock's clock error bound and network RTT, and an inverse relationship
    /// with the time between the two events.
    ///
    /// Over the steady state, the FF algorithm will use data points which are minutes apart. This will
    /// make the reference clock's clock error bound and RTT values statistically insignificant.
    ///
    /// However, after a disruption event the effects from the clock error bound and RTT can become more pronounced.
    /// This calculation stays honest with that.
    ///
    /// # Panics
    /// Panics if the two events are equal
    pub fn calculate_period_with_error(&self, other: &Self) -> LocalPeriodAndError {
        let (old, new) = if self.counter_pre < other.counter_pre {
            (self, other)
        } else {
            (other, self)
        };

        // Unit-less error values
        let period_error_from_ceb = (old.data.clock_error_bound + new.data.clock_error_bound)
            .as_seconds_f64()
            / (new.data.time - old.data.time).as_seconds_f64();
        #[allow(
            clippy::cast_precision_loss,
            reason = "Durations will be a max of 2 weeks. Precision loss is minimized"
        )]
        let period_error_from_rtt = (old.rtt() + new.rtt()).get() as f64
            / (2.0 * (new.tsc_midpoint() - old.tsc_midpoint()).get() as f64);

        let period = self.calculate_period(other);

        // Calculates the "steepest" possible slope based off of the error bounding boxes
        let period_shrink =
            period.get() * ((1.0 + period_error_from_ceb) / (1.0 - period_error_from_rtt));

        // Error is the difference of the two slopes
        let error = (period.get() - period_shrink).abs();
        let error = Period::from_seconds(error);

        LocalPeriodAndError {
            period_local: period,
            error,
        }
    }

    /// Calculate the clock error bound of this event at the time of the event
    ///
    /// This is different from the clock error bound that would be reported to a user outside of the daemon.
    ///
    /// First because this is a sans-IO input, there is not concept of reading this "after" the event comes in.
    /// Because of this, there is no additional value added to the root-dispersion.
    ///
    /// Second, the round trip time needs a calculations of the period to be able to convert the TSC rtt into a
    /// duration of time.
    ///
    /// Third, there is no "system clock offset" value. That is a parameter exclusive to modifying the system clock, which this component does not do.
    pub fn calculate_clock_error_bound(&self, period_local: Period) -> Duration {
        let rtt = self.rtt() * period_local;
        self.data().clock_error_bound + (rtt / 2)
    }

    /// Calculate offset using the uncorrected clock
    ///
    /// Offset is positive if the client is ahead of the reference clock
    pub fn calculate_offset(&self, uncorrected_clock: UncorrectedClock) -> Duration {
        let client_midpoint = self.tsc_midpoint();
        let client_midpoint = uncorrected_clock.time_at(client_midpoint);

        client_midpoint - self.data().time
    }
}

impl TscRtt for Phc {
    fn counter_pre(&self) -> TscCount {
        self.counter_pre
    }

    fn counter_post(&self) -> TscCount {
        self.counter_post
    }
}

/// PHC specific data
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhcData {
    /// Reference clock time
    pub time: Instant,
    /// Clock error bound of this measurement
    pub clock_error_bound: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn phc_invalid() {
        let result = Phc::builder()
            .counter_post(TscCount::new(5))
            .counter_pre(TscCount::new(10)) // pre after post
            .data(PhcData {
                time: Instant::from_days(5),
                clock_error_bound: Duration::from_micros(15),
            })
            .build();

        assert!(result.is_none());
    }

    #[test]
    fn phc_valid() {
        let _result = Phc::builder()
            .counter_post(TscCount::new(10))
            .counter_pre(TscCount::new(5)) // pre before post
            .data(PhcData {
                time: Instant::from_days(5),
                clock_error_bound: Duration::from_micros(15),
            })
            .build()
            .unwrap();
    }

    #[rstest]
    #[case::minimal_delays(
        Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_002_000))
            .data(PhcData {
                time: Instant::from_days(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(11),
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Duration::from_micros(12)
    )]
    #[case::larger_rtt(
        Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_010_000))
            .data(PhcData {
                time: Instant::from_days(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(30),
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Duration::from_micros(35)
    )]
    #[case::period_scaling(
        Phc::builder()
            .counter_pre(TscCount::new(2_000_000_000))
            .counter_post(TscCount::new(2_000_002_000))
            .data(PhcData {
                time: Instant::from_days(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_micros(23),
            })
            .build()
            .unwrap(),
        Period::from_seconds(2e-9),  // Different period scaling
        Duration::from_nanos(25_000),
    )]
    fn calculate_clock_error_bound(
        #[case] event: Phc,
        #[case] period: Period,
        #[case] expected: Duration,
    ) {
        let result = event.calculate_clock_error_bound(period);
        approx::assert_abs_diff_eq!(
            result.as_seconds_f64(),
            expected.as_seconds_f64(),
            epsilon = 1e-9
        );
    }

    fn create_phc_event(pre: TscCount, post: TscCount, server_time: Instant) -> Phc {
        Phc::builder()
            .counter_pre(pre)
            .counter_post(post)
            .data(PhcData {
                time: server_time,
                clock_error_bound: Duration::from_nanos(0), // Not used in calculation           // Not used in calculation
            })
            .build()
            .unwrap()
    }

    #[rstest]
    #[case(
        // First event
        (TscCount::new(100), TscCount::new(200), Instant::from_days(1000)),
        // Second event
        (TscCount::new(300), TscCount::new(400), Instant::from_days(1000) + Duration::from_secs(1)),
        Period::from_seconds(0.005),
    )]
    #[case(
        // First event
        (TscCount::new(1000), TscCount::new(2000), Instant::from_days(0)),
        // Second event
        (TscCount::new(3000), TscCount::new(4000), Instant::from_millis(500)),
        Period::from_seconds(0.00025),
    )]
    #[case(
        // First event with larger values
        (TscCount::new(10000), TscCount::new(20000), Instant::from_secs(100000)),
        // Second event
        (TscCount::new(30000), TscCount::new(40000), Instant::from_secs(200000)),
        // Expected period (server_time_diff / tsc_diff = (200000-100000)/(40000-20000) = 5)
        Period::from_seconds(5.0),
    )]
    fn test_calculate_period_backward(
        #[case] (first_pre, first_post, first_send): (TscCount, TscCount, Instant),
        #[case] (second_pre, second_post, second_send): (TscCount, TscCount, Instant),
        #[case] expected_period: Period,
    ) {
        let event1 = create_phc_event(first_pre, first_post, first_send);
        let event2 = create_phc_event(second_pre, second_post, second_send);

        let period = event1.calculate_period(&event2);
        approx::assert_abs_diff_eq!(period.get(), expected_period.get());
    }

    #[rstest]
    #[case(
        // Zero root delay and dispersion
        Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_002_000))
            .data(PhcData {
                time: Instant::from_days(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_nanos(0),
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Duration::from_micros(1)
    )]
    #[case(
        // Large root delay and dispersion
        Phc::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_001_000))
            .data(PhcData {
                time: Instant::from_days(1) + Duration::from_nanos(500),
                clock_error_bound: Duration::from_millis(2),
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Duration::from_nanos(2_000_500),
    )]
    fn calculate_clock_error_bound_edge_cases(
        #[case] event: Phc,
        #[case] period: Period,
        #[case] expected: Duration,
    ) {
        let result = event.calculate_clock_error_bound(period);
        approx::assert_abs_diff_eq!(
            result.as_seconds_f64(),
            expected.as_seconds_f64(),
            epsilon = 1e-9
        );
    }

    // Helper function to create an UncorrectedClock with specific parameters
    fn create_uncorrected_clock(k: Instant, p_estimate: Period) -> UncorrectedClock {
        UncorrectedClock { k, p_estimate }
    }

    #[rstest]
    #[case::client_ahead(
        // Test case where client is ahead of server
        Phc::builder()
            .counter_pre(TscCount::new(1000))
            .counter_post(TscCount::new(2000))
            .data(PhcData {
                time: Instant::from_millis(10500),
                clock_error_bound: Duration::from_secs(0),
            })
            .build()
            .unwrap(),
        create_uncorrected_clock(
            Instant::from_secs(0),
            Period::from_seconds(0.02) // 20ms per tick
        ),
        Duration::from_seconds_f64(19.5) // Expected positive offset
    )]
    #[case::client_behind(
        // Test case where client is behind server
        Phc::builder()
            .counter_pre(TscCount::new(1000))
            .counter_post(TscCount::new(2000))
            .data(PhcData {
                time: Instant::from_millis(50500),
                clock_error_bound: Duration::from_secs(0),
            })
            .build()
            .unwrap(),
        create_uncorrected_clock(
            Instant::from_secs(0),
            Period::from_seconds(0.02) // 20ms per tick
        ),
        Duration::from_seconds_f64(-20.5) // Expected negative offset
    )]
    #[case::zero_offset(
        // Test case where client and server are synchronized
        Phc::builder()
            .counter_pre(TscCount::new(1000))
            .counter_post(TscCount::new(2000))
            .data(PhcData {
                time: Instant::from_millis(25000),
                clock_error_bound: Duration::from_secs(0),
            })
            .build()
            .unwrap(),
        create_uncorrected_clock(
            Instant::from_secs(10),
            Period::from_seconds(0.01) // 10ms per tick
        ),
        Duration::from_secs(0) // Expected zero offset
    )]
    fn calculate_offset(
        #[case] phc_event: Phc,
        #[case] uncorrected_clock: UncorrectedClock,
        #[case] expected_offset: Duration,
    ) {
        let client_midpoint = phc_event.counter_pre.midpoint(phc_event.counter_post);
        println!(
            "counter_pre: {:?}",
            uncorrected_clock.time_at(phc_event.counter_pre)
        );
        println!(
            "counter_post: {:?}",
            uncorrected_clock.time_at(phc_event.counter_post)
        );
        let client_midpoint = uncorrected_clock.time_at(client_midpoint);
        println!("client_midpoint: {client_midpoint:?}");
        let offset = phc_event.calculate_offset(uncorrected_clock);

        approx::assert_abs_diff_eq!(
            offset.as_seconds_f64(),
            expected_offset.as_seconds_f64(),
            epsilon = 1e-9
        );
    }

    fn create_phc_event_with_error(
        pre: TscCount,
        post: TscCount,
        server_time: Instant,
        error: Duration,
    ) -> Phc {
        Phc::builder()
            .counter_pre(pre)
            .counter_post(post)
            .data(PhcData {
                time: server_time,
                clock_error_bound: error,
            })
            .build()
            .unwrap()
    }

    // grabbed data from atss (ntp. But math is the same as phc)
    #[rstest]
    #[case::first_two(
        // First event
        (TscCount::new(1369766986771638), TscCount::new(1369766987268186), Instant::from_nanos(1763156375539567199), Duration::from_nanos(15259)),
        // Second event
        (TscCount::new(1369767115896036), TscCount::new(1369767116312166), Instant::from_nanos(1763156375589220795), Duration::from_nanos(15259)),
        Period::from_seconds(3.84660556685218820E-10),
        Period::from_seconds(1.601932955446111e-12),
    )]
    #[case::longer_term(
        // First event
        (TscCount::new(1372612880990286), TscCount::new(1372612881496636), Instant::from_nanos(1763157470124988894), Duration::from_nanos(30518)),
        // Second event
        (TscCount::new(1372984678771576), TscCount::new(1372984679237314), Instant::from_nanos(1763157613125523830), Duration::from_nanos(15259)),
        Period::from_seconds(3.846191396030325e-10),
        Period::from_seconds(6.259276438100348e-16),
    )]
    #[case::backward(
        // Second event
        (TscCount::new(1369767115896036), TscCount::new(1369767116312166), Instant::from_nanos(1763156375589220795), Duration::from_nanos(15259)),
        // First event
        (TscCount::new(1369766986771638), TscCount::new(1369766987268186), Instant::from_nanos(1763156375539567199), Duration::from_nanos(15259)),
        Period::from_seconds(3.84660556685218820E-10),
        Period::from_seconds(1.601932955446111e-12),
    )]

    fn test_calculate_period_with_error(
        #[case] (first_pre, first_post, first_send, first_ceb): (
            TscCount,
            TscCount,
            Instant,
            Duration,
        ),
        #[case] (second_pre, second_post, second_send, second_ceb): (
            TscCount,
            TscCount,
            Instant,
            Duration,
        ),
        #[case] expected_period: Period,
        #[case] expected_period_error: Period,
    ) {
        let event1 = create_phc_event_with_error(first_pre, first_post, first_send, first_ceb);
        let event2 = create_phc_event_with_error(second_pre, second_post, second_send, second_ceb);

        let res = event1.calculate_period_with_error(&event2);
        approx::assert_abs_diff_eq!(res.period_local.get(), expected_period.get());
        approx::assert_abs_diff_eq!(res.error.get(), expected_period_error.get());
    }
}
