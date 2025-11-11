//! PHC Time synchronization events

use crate::daemon::{
    clock_sync_algorithm::ff::UncorrectedClock,
    time::{Duration, Instant, TscCount, tsc::Period},
};

use super::TscRtt;

/// Contains the PHC and time stamp counter samples to be used by the synchronization algorithm
///
/// `tsc_post` must be greater than `tsc_pre`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Phc {
    /// TSC value before reading reference clock
    tsc_pre: TscCount,
    /// TSC value after reading reference clock
    tsc_post: TscCount,
    /// PHC reference clock info
    data: PhcData,
    #[cfg(all(not(test), feature = "test-side-by-side"))]
    #[serde(skip)]
    system_clock: Option<super::SystemClockMeasurement>,
}

#[bon::bon]
impl Phc {
    /// Construct a [`Phc`]
    ///
    /// Returns `None` if `tsc_post <= tsc_pre`
    #[builder]
    pub fn new(
        tsc_pre: TscCount,
        tsc_post: TscCount,
        data: PhcData,
        #[cfg(all(not(test), feature = "test-side-by-side"))] system_clock: Option<
            super::SystemClockMeasurement,
        >,
    ) -> Option<Self> {
        if tsc_post <= tsc_pre {
            return None;
        }

        Some(Self {
            tsc_pre,
            tsc_post,
            data,
            #[cfg(all(not(test), feature = "test-side-by-side"))]
            system_clock,
        })
    }
}

impl Phc {
    /// `tsc_pre` getter
    pub fn tsc_pre(&self) -> TscCount {
        self.tsc_pre
    }

    /// `tsc_post` getter
    pub fn tsc_post(&self) -> TscCount {
        self.tsc_post
    }

    /// phc data getter
    pub fn data(&self) -> &PhcData {
        &self.data
    }

    /// system time getter
    #[cfg(all(not(test), feature = "test-side-by-side"))]
    pub fn system_clock(&self) -> Option<&super::SystemClockMeasurement> {
        self.system_clock.as_ref()
    }

    /// Calculate a period by using 2 PHC events using the return path
    ///
    /// PHC reads are characterized in ClockBound with each read having
    /// - `tsc_pre`: The TSC reading before reading the PHC device
    /// - `time`: The PHC time reading
    /// - `tsc_post`: The TSC reading after reading the PHC device
    ///
    /// The "backward" path here means using the `tsc_post` and `time` from the events
    ///
    /// # Panics
    /// Panics if the events are the same (specifically if the `tsc_post` values are equal)
    pub fn calculate_period_backward(&self, other: &Self) -> Period {
        (self.data().time - other.data().time) / (self.tsc_post() - other.tsc_post())
    }

    /// Calculate the clock error bound of this event at the time of the event
    ///
    /// This is different from the clock error bound that would be reported to a user outside of hte daemon.
    ///
    /// First because this is a sans-IO input, there is not concept of reading this "after" the event comes in.
    /// Because of this, there is no additional value added to the root-dispersion.
    ///
    /// Second, the round trip time needs a calculations of hte period to be able to convert hte TSC rtt into a
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
    fn tsc_pre(&self) -> TscCount {
        self.tsc_pre
    }

    fn tsc_post(&self) -> TscCount {
        self.tsc_post
    }
}

/// PHC specific data
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
            .tsc_post(TscCount::new(5))
            .tsc_pre(TscCount::new(10)) // pre after post
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
            .tsc_post(TscCount::new(10))
            .tsc_pre(TscCount::new(5)) // pre before post
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
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_002_000))
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
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_010_000))
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
            .tsc_pre(TscCount::new(2_000_000_000))
            .tsc_post(TscCount::new(2_000_002_000))
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
            .tsc_pre(pre)
            .tsc_post(post)
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

        let period = event1.calculate_period_backward(&event2);
        approx::assert_abs_diff_eq!(period.get(), expected_period.get());
    }

    #[rstest]
    #[case(
        // Zero root delay and dispersion
        Phc::builder()
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_002_000))
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
            .tsc_pre(TscCount::new(1_000_000_000))
            .tsc_post(TscCount::new(1_000_001_000))
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
            .tsc_pre(TscCount::new(1000))
            .tsc_post(TscCount::new(2000))
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
            .tsc_pre(TscCount::new(1000))
            .tsc_post(TscCount::new(2000))
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
            .tsc_pre(TscCount::new(1000))
            .tsc_post(TscCount::new(2000))
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
        #[case] ntp_event: Phc,
        #[case] uncorrected_clock: UncorrectedClock,
        #[case] expected_offset: Duration,
    ) {
        let client_midpoint = ntp_event.tsc_pre.midpoint(ntp_event.tsc_post);
        println!(
            "tsc_pre: {:?}",
            uncorrected_clock.time_at(ntp_event.tsc_pre)
        );
        println!(
            "tsc_post: {:?}",
            uncorrected_clock.time_at(ntp_event.tsc_post)
        );
        let client_midpoint = uncorrected_clock.time_at(client_midpoint);
        println!("client_midpoint: {client_midpoint:?}");
        let offset = ntp_event.calculate_offset(uncorrected_clock);

        approx::assert_abs_diff_eq!(
            offset.as_seconds_f64(),
            expected_offset.as_seconds_f64(),
            epsilon = 1e-9
        );
    }
}
