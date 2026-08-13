//! NTP Time synchronization events
use std::{
    error::Error,
    fmt::{Display, Formatter},
};

use super::TscRtt;
use crate::daemon::{
    clock_sync_algorithm::ff::{LocalPeriodAndError, UncorrectedClock},
    time::{Duration, Instant, TscCount, tsc::Period},
};

/// Contains the NTP and time stamp counter samples to be used by synchronization algorithm.
///
/// `counter_post` must be greater than `counter_pre`
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Ntp {
    /// TSC value before sending event
    #[serde(alias = "tsc_pre")] // for backwards compatibility on ff-tester repro
    counter_pre: TscCount,
    /// TSC value after sending event
    #[serde(alias = "tsc_post")] // for backwards compatibility on ff-tester repro
    counter_post: TscCount,
    /// NTP Packet data
    data: NtpData,
    #[cfg(not(test))]
    #[serde(skip)]
    system_clock: Option<super::SystemClockMeasurement>,
}

#[bon::bon]
impl Ntp {
    /// Construct a [`Ntp`]
    ///
    /// Returns `None` if `counter_post <= counter_pre`
    #[builder]
    pub fn new(
        counter_pre: TscCount,
        counter_post: TscCount,
        ntp_data: NtpData,
        #[cfg(not(test))] system_clock: Option<super::SystemClockMeasurement>,
    ) -> Option<Self> {
        if counter_post > counter_pre {
            Some(Self {
                counter_pre,
                counter_post,
                data: ntp_data,
                #[cfg(not(test))]
                system_clock,
            })
        } else {
            None
        }
    }
}

impl Ntp {
    /// `counter_pre` getter
    pub fn counter_pre(&self) -> TscCount {
        self.counter_pre
    }

    /// `counter_post` getter
    pub fn counter_post(&self) -> TscCount {
        self.counter_post
    }

    /// `data` getter
    pub fn data(&self) -> &NtpData {
        &self.data
    }

    /// system time getter
    #[cfg(not(test))]
    pub fn system_clock(&self) -> Option<&super::SystemClockMeasurement> {
        self.system_clock.as_ref()
    }

    /// Calculate a period by using 2 NTP events using midpoints
    ///
    /// NTP traffic is characterized in ClockBound with each exchange having a
    /// - `counter_pre`: The TSC reading before sending the NTP packet
    /// - `server_recv_system_time`: The server's system time after receiving the NTP packet
    /// - `server_send_system_time`: The server's system time after sending the NTP packet
    /// - `counter_post`: The TSC reading after sending the NTP packet
    ///
    /// # Panics
    /// - Panics if events share the same tsc midpoint (happens if the events are the same).
    /// - Panics if events share the same server time midpoint (also happens if the events are the same)
    pub fn calculate_period(&self, other: &Self) -> Period {
        let self_server_midpoint = self
            .data()
            .server_recv_time
            .midpoint(self.data().server_send_time);
        let self_tsc_midpoint = self.tsc_midpoint();

        let other_server_midpoint = other
            .data()
            .server_recv_time
            .midpoint(other.data().server_send_time);
        let other_tsc_midpoint = other.tsc_midpoint();

        (self_server_midpoint - other_server_midpoint) / (self_tsc_midpoint - other_tsc_midpoint)
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
    pub fn calculate_period_with_error(&self, other: &Self) -> LocalPeriodAndError {
        let (old, new) = if self.counter_pre < other.counter_pre {
            (self, other)
        } else {
            (other, self)
        };

        // This is the server reported clock error bound. Includes neither peer delay nor local dispersion
        let old_server_ceb = old.data.root_dispersion + old.data.root_delay / 2;
        let new_server_ceb = new.data.root_dispersion + new.data.root_delay / 2;

        let old_server_midpoint = old
            .data
            .server_recv_time
            .midpoint(old.data.server_send_time);
        let new_server_midpoint = new
            .data
            .server_recv_time
            .midpoint(new.data.server_send_time);

        // Unit-less error values
        let period_error_from_ceb = (old_server_ceb + new_server_ceb).as_seconds_f64()
            / (new_server_midpoint - old_server_midpoint).as_seconds_f64();
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
    /// First, because this is a sans-IO input, there is no concept of reading this "after" the event comes in.
    /// Because of this, there is no additional value added to the root-dispersion.
    ///
    /// Second, the round trip time needs a calculation of the period to be able to convert the TSC rtt into
    /// a duration of time.
    ///
    /// Third, there is no "ntp offset" value. That is a parameter exclusive to modifying the system clock, which this component does not do.
    /// Instead it just calculates the time at a TSC event, and then passes that on to the [`ClockState`](crate::daemon::clock_state) component.
    pub fn calculate_clock_error_bound(&self, period_local: Period) -> Duration {
        let rtt = self.rtt() * period_local;
        let root_delay = self.data().root_delay + rtt;
        self.data().root_dispersion + (root_delay / 2)
    }

    /// Calculate offset using the uncorrected clock
    ///
    /// Offset is positive if the client is ahead of the server
    pub fn calculate_offset(&self, uncorrected_clock: UncorrectedClock) -> Duration {
        // calculate midpoints on client and server side
        let client_send_time = uncorrected_clock.time_at(self.counter_pre());
        let client_recv_time = uncorrected_clock.time_at(self.counter_post());
        let client_midpoint = client_send_time.midpoint(client_recv_time);

        let server = self.data();
        let server_midpoint = server.server_recv_time.midpoint(server.server_send_time);

        // calculate the uncorrected offset from the reference clock
        // offset is positive if local clock is ahead of server
        client_midpoint - server_midpoint
    }
}

impl TscRtt for Ntp {
    fn counter_pre(&self) -> TscCount {
        self.counter_pre
    }

    fn counter_post(&self) -> TscCount {
        self.counter_post
    }
}

/// NTP-specific data
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NtpData {
    /// NTP Server recv time
    #[serde(rename = "server_recv_time_ns")]
    pub server_recv_time: Instant,
    /// NTP Server send time
    #[serde(rename = "server_send_time_ns")]
    pub server_send_time: Instant,

    /// Root Delay of NTP packet
    #[serde(rename = "root_delay_ns")]
    pub root_delay: Duration,
    /// Root Dispersion of NTP packet
    #[serde(rename = "root_dispersion_ns")]
    pub root_dispersion: Duration,

    /// NTP Stratum. Used in reporting, not used in ff-sync
    pub stratum: Stratum,
}

/// An NTP stratum
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "u8", into = "u8")]
pub enum Stratum {
    /// Unspecified or invalid.
    ///
    /// Corresponds to a value of 0 in an NTP packet
    Unspecified,
    /// A server stratum level
    ///
    /// Corresponds to a value of 1-15 in an NTP packet
    Level(ValidStratumLevel),
    /// Clock is unsynchronized
    ///
    /// Corresponds to a value of 16 in an NTP packet
    Unsynchronized,
}

impl Stratum {
    /// Stratum 1
    pub const ONE: Self = Self::Level(ValidStratumLevel(1));

    /// Stratum 2
    pub const TWO: Self = Self::Level(ValidStratumLevel(2));

    /// Construct a new stratum from a `u8` value
    ///
    /// Returns none if the value is > 16
    pub const fn new(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Unspecified),
            16 => Some(Self::Unsynchronized),
            1..=15 => match ValidStratumLevel::new(value) {
                Some(level) => Some(Self::Level(level)),
                None => None,
            },
            _ => None,
        }
    }

    /// Get the incremented stratum for this NTP client
    ///
    /// Returns this stratum + 1, capped at `Unsynchronized` (16).
    ///
    /// # Panics
    /// Never panics - all incremented values are guaranteed to be valid.
    #[must_use]
    pub fn incremented(&self) -> Stratum {
        let current_value = u8::from(*self);
        match current_value {
            0..=14 => Stratum::Level(
                ValidStratumLevel::new(current_value + 1)
                    .expect("value 1-15 should be valid stratum level"),
            ),
            _ => Stratum::Unsynchronized,
        }
    }
}

impl From<Stratum> for u8 {
    fn from(stratum: Stratum) -> Self {
        match stratum {
            Stratum::Unspecified => 0,
            Stratum::Level(level) => level.get(),
            Stratum::Unsynchronized => 16,
        }
    }
}

impl TryFrom<u8> for Stratum {
    type Error = TryFromU8Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Stratum::new(value).ok_or(TryFromU8Error)
    }
}

/// The error type returned when a checked integral type conversion fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TryFromU8Error;

impl Display for TryFromU8Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid value")
    }
}

impl Error for TryFromU8Error {}

/// A valid stratum level, from 1 to 15
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValidStratumLevel(u8);

impl ValidStratumLevel {
    /// Create a new valid stratum level
    /// Returns None if the value is not between 1 and 15
    pub const fn new(value: u8) -> Option<Self> {
        if value > 0 && value <= 15 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Get the inner value
    pub fn get(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(Stratum::Unspecified, Stratum::Level(ValidStratumLevel::new(1).unwrap()))]
    #[case(Stratum::ONE, Stratum::TWO)]
    #[case(Stratum::TWO, Stratum::Level(ValidStratumLevel::new(3).unwrap()))]
    #[case(Stratum::Level(ValidStratumLevel::new(14).unwrap()), Stratum::Level(ValidStratumLevel::new(15).unwrap()))]
    #[case(Stratum::Level(ValidStratumLevel::new(15).unwrap()), Stratum::Unsynchronized)]
    #[case(Stratum::Unsynchronized, Stratum::Unsynchronized)]
    fn stratum_incremented(#[case] input: Stratum, #[case] expected: Stratum) {
        assert_eq!(input.incremented(), expected);
    }

    #[test]
    fn valid_ntp_event() {
        let event = Ntp::builder()
            .counter_pre(TscCount::new(1))
            .counter_post(TscCount::new(2))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1),
                server_send_time: Instant::new(2),
                root_delay: Duration::new(3),
                root_dispersion: Duration::new(4),
                stratum: Stratum::ONE,
            })
            .build();

        let event = event.unwrap();

        assert_eq!(event.counter_pre().get(), 1);
        assert_eq!(event.counter_post().get(), 2);
        assert_eq!(event.data().server_recv_time, Instant::new(1));
        assert_eq!(event.data().server_send_time, Instant::new(2));
        assert_eq!(event.data().root_delay, Duration::new(3));
        assert_eq!(event.data().root_dispersion, Duration::new(4));
        assert_eq!(event.data().stratum, Stratum::ONE);
    }

    #[test]
    fn wrong_tsc_order() {
        let event = Ntp::builder()
            .counter_pre(TscCount::new(2))
            .counter_post(TscCount::new(1))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1),
                server_send_time: Instant::new(2),
                root_delay: Duration::new(3),
                root_dispersion: Duration::new(4),
                stratum: Stratum::ONE,
            })
            .build();

        assert!(event.is_none());
    }

    #[test]
    fn stratum_new_valid_values() {
        assert_eq!(Stratum::new(0), Some(Stratum::Unspecified));
        assert_eq!(Stratum::new(16), Some(Stratum::Unsynchronized));

        // Test valid levels 1-15
        for i in 1..=15 {
            let stratum = Stratum::new(i);
            assert!(stratum.is_some());
            let Some(Stratum::Level(level)) = stratum else {
                panic!("Expected Stratum::Level for value {}", i);
            };
            assert_eq!(level.get(), i);
        }
    }

    #[test]
    fn stratum_new_invalid_values() {
        assert_eq!(Stratum::new(17), None);
        assert_eq!(Stratum::new(255), None);
    }

    #[test]
    fn stratum_conversion_to_u8() {
        assert_eq!(u8::from(Stratum::Unspecified), 0);
        assert_eq!(u8::from(Stratum::Unsynchronized), 16);

        // Test conversion of valid levels
        for i in 1..=15 {
            let level = ValidStratumLevel::new(i).unwrap();
            assert_eq!(u8::from(Stratum::Level(level)), i);
        }
    }

    #[test]
    fn stratum_try_from_u8() {
        // Test valid conversions
        assert!(matches!(Stratum::try_from(0), Ok(Stratum::Unspecified)));
        assert!(matches!(Stratum::try_from(16), Ok(Stratum::Unsynchronized)));

        // Test valid levels
        for i in 1..=15 {
            let result = Stratum::try_from(i);
            assert!(result.is_ok());
            assert!(matches!(result.unwrap(), Stratum::Level(_)));
        }
    }

    #[test]
    fn invalid_try_from_u8() {
        // Test invalid conversions
        assert!(matches!(Stratum::try_from(17), Err(TryFromU8Error)));
        assert!(matches!(Stratum::try_from(255), Err(TryFromU8Error)));
    }

    fn create_ntp_event(pre: TscCount, post: TscCount, server_time: Instant) -> Ntp {
        let server_duration = Duration::from_micros(40);
        Ntp::builder()
            .counter_pre(pre)
            .counter_post(post)
            .ntp_data(NtpData {
                server_recv_time: server_time - (server_duration / 2),
                server_send_time: server_time + (server_duration / 2),
                root_delay: Duration::from_nanos(0), // Not used in calculation
                root_dispersion: Duration::from_nanos(0), // Not used in calculation
                stratum: Stratum::ONE,               // Not used in calculation
            })
            .build()
            .unwrap()
    }

    #[rstest]
    #[case::minimal_delays(
        Ntp::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_002_000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(1),
                root_delay: Duration::from_micros(10),
                root_dispersion: Duration::from_micros(5),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Duration::from_micros(11)  // Expected: root_dispersion(5) + (root_delay(10) + rtt(2))/2
    )]
    #[case::larger_rtt(
        Ntp::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_010_000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(1),
                root_delay: Duration::from_micros(20),
                root_dispersion: Duration::from_micros(10),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Duration::from_micros(25)  // Expected: root_dispersion(10) + (root_delay(20) + rtt(10))/2 
    )]
    #[case::period_scaling(
        Ntp::builder()
            .counter_pre(TscCount::new(2_000_000_000))
            .counter_post(TscCount::new(2_000_002_000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(1),
                root_delay: Duration::from_micros(15),
                root_dispersion: Duration::from_micros(8),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(2e-9),  // Different period scaling
        Duration::from_nanos(17_500)  // Expected: root_dispersion(8) + (root_delay(15) + rtt(4))/2
    )]
    fn calculate_clock_error_bound(
        #[case] event: Ntp,
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
    fn test_calculate_period(
        #[case] (first_pre, first_post, first_send): (TscCount, TscCount, Instant),
        #[case] (second_pre, second_post, second_send): (TscCount, TscCount, Instant),
        #[case] expected_period: Period,
    ) {
        let event1 = create_ntp_event(first_pre, first_post, first_send);
        let event2 = create_ntp_event(second_pre, second_post, second_send);

        let period = event1.calculate_period(&event2);
        approx::assert_abs_diff_eq!(period.get(), expected_period.get());
    }

    #[rstest]
    #[case(
        // Zero root delay and dispersion
        Ntp::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_002_000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(1),
                root_delay: Duration::from_micros(0),
                root_dispersion: Duration::from_micros(0),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Duration::from_micros(1)  // Expected: only RTT contribution
    )]
    #[case(
        // Large root delay and dispersion
        Ntp::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_001_000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + Duration::from_micros(1),
                root_delay: Duration::from_millis(1),
                root_dispersion: Duration::from_millis(1),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap(),
        Period::from_seconds(1e-9),
        Duration::from_nanos(1_500_500)  // Expected: root_dispersion(1ms) + (root_delay(1ms) + rtt(1µs))/2
    )]
    fn calculate_clock_error_bound_edge_cases(
        #[case] event: Ntp,
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
        Ntp::builder()
            .counter_pre(TscCount::new(1000))
            .counter_post(TscCount::new(2000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_secs(10),
                server_send_time: Instant::from_secs(11),
                root_delay: Duration::from_secs(0),
                root_dispersion: Duration::from_secs(0),
                stratum: Stratum::ONE,
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
        Ntp::builder()
            .counter_pre(TscCount::new(1000))
            .counter_post(TscCount::new(2000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_secs(50),
                server_send_time: Instant::from_secs(51),
                root_delay: Duration::from_secs(0),
                root_dispersion: Duration::from_secs(0),
                stratum: Stratum::ONE,
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
        Ntp::builder()
            .counter_pre(TscCount::new(1000))
            .counter_post(TscCount::new(2000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_secs(20),
                server_send_time: Instant::from_secs(30),
                root_delay: Duration::from_secs(0),
                root_dispersion: Duration::from_secs(0),
                stratum: Stratum::ONE,
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
        #[case] ntp_event: Ntp,
        #[case] uncorrected_clock: UncorrectedClock,
        #[case] expected_offset: Duration,
    ) {
        let client_midpoint = ntp_event.counter_pre.midpoint(ntp_event.counter_post);
        println!(
            "counter_pre: {:?}",
            uncorrected_clock.time_at(ntp_event.counter_pre)
        );
        println!(
            "counter_post: {:?}",
            uncorrected_clock.time_at(ntp_event.counter_post)
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

    fn create_ntp_event_with_error(
        pre: TscCount,
        post: TscCount,
        server_time: Instant,
        server_time_error: Duration,
    ) -> Ntp {
        let server_duration = Duration::from_micros(40);
        Ntp::builder()
            .counter_pre(pre)
            .counter_post(post)
            .ntp_data(NtpData {
                server_recv_time: server_time - (server_duration / 2),
                server_send_time: server_time + (server_duration / 2),
                root_delay: Duration::from_nanos(0), // Kinda not used in calculation
                root_dispersion: server_time_error,
                stratum: Stratum::ONE, // Not used in calculation
            })
            .build()
            .unwrap()
    }

    // grabbed data from atss
    #[rstest]
    #[case::first_two_burst(
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
        let event1 = create_ntp_event_with_error(first_pre, first_post, first_send, first_ceb);
        let event2 = create_ntp_event_with_error(second_pre, second_post, second_send, second_ceb);

        let res = event1.calculate_period_with_error(&event2);
        approx::assert_abs_diff_eq!(res.period_local.get(), expected_period.get());
        approx::assert_abs_diff_eq!(res.error.get(), expected_period_error.get());
    }
}
