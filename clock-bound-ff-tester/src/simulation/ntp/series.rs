//! An ntp source pre-configurable with a series of values for key variables
//! such as rood delay, root dispersion, and rtt variables

use crate::simulation::{interpolation::SeriesInterpolation, oscillator::FullModel};

use super::round_trip_delays::{NtpEventTimestamps, RoundTripDelays};
use crate::events::v1::{Event, EventKind, Ntp};
use crate::time::{AssumeTrue, DemoteToEstimate, EstimateDuration, Series, TrueDuration, TscCount};
use thiserror;

/// Error types for NTP generator validation and initialization
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Series {data_series} must have at least {min} points, found {actual}.")]
    TooFewDataPoints {
        data_series: &'static str,
        min: usize,
        actual: usize,
    },
    #[error(
        "Series {data_series} must start at simulation time {required:?}, found first data point at {first_data_tstamp:?}."
    )]
    IllegalStartTime {
        data_series: &'static str,
        required: TrueDuration,
        first_data_tstamp: TrueDuration,
    },
    #[error(
        "Series {data_series} must span at least one polling period of {poll_period:?}, found last data point at {last_data_tstamp:?}."
    )]
    IllegalEndTime {
        data_series: &'static str,
        poll_period: EstimateDuration,
        last_data_tstamp: TrueDuration,
    },
    #[error("Poll period must be at minimum {min:?}, found {actual:?}.")]
    IllegalPollPeriod {
        min: EstimateDuration,
        actual: EstimateDuration,
    },
}

/// Holds a time series of network forward delays from client to server
#[derive(Debug, Clone)]
pub struct ForwardDelays {
    pub inner: Series<TrueDuration, TrueDuration>,
}

impl ForwardDelays {
    /// Create a new forward delay series
    pub fn new(inner: Series<TrueDuration, TrueDuration>) -> Self {
        Self { inner }
    }

    /// Return the time series indices
    pub fn indices(&self) -> &[TrueDuration] {
        self.inner.indices()
    }

    /// Return the forward delays of the time series
    pub fn data(&self) -> &[TrueDuration] {
        self.inner.data()
    }

    /// Return the approximate data at the given time index
    pub fn approximate(&self, x: TrueDuration) -> Option<TrueDuration> {
        self.inner.approximate(x)
    }
}

/// Holds a time series of network backward delays from server to client
#[derive(Debug, Clone)]
pub struct BackwardDelays {
    pub inner: Series<TrueDuration, TrueDuration>,
}

impl BackwardDelays {
    /// Create a new backward delay series
    pub fn new(inner: Series<TrueDuration, TrueDuration>) -> Self {
        Self { inner }
    }

    /// Return the time series indices
    pub fn indices(&self) -> &[TrueDuration] {
        self.inner.indices()
    }

    /// Return the backward delays of the time series
    pub fn data(&self) -> &[TrueDuration] {
        self.inner.data()
    }

    /// Return the approximate data at the given time index
    pub fn approximate(&self, x: TrueDuration) -> Option<TrueDuration> {
        self.inner.approximate(x)
    }
}

/// Holds a time series of server processing delays
#[derive(Debug, Clone)]
pub struct ServerDelays {
    pub inner: Series<TrueDuration, TrueDuration>,
}

impl ServerDelays {
    /// Create a new server delay series
    pub fn new(inner: Series<TrueDuration, TrueDuration>) -> Self {
        Self { inner }
    }

    /// Return the time series indices
    pub fn indices(&self) -> &[TrueDuration] {
        self.inner.indices()
    }

    /// Return the server delays of the time series
    pub fn data(&self) -> &[TrueDuration] {
        self.inner.data()
    }

    /// Return the approximate data at the given time index
    pub fn approximate(&self, x: TrueDuration) -> Option<TrueDuration> {
        self.inner.approximate(x)
    }
}

/// Configuration properties for the time series NTP generator
///
/// Contains all the time series that define how NTP variables change over time,
/// including root delays, root dispersions, and round-trip delay components.
#[derive(Debug, Clone)]
pub struct Props {
    pub poll_period: EstimateDuration,
    pub root_delays: Series<TrueDuration, EstimateDuration>,
    pub root_dispersions: Series<TrueDuration, EstimateDuration>,
    pub rtt_backwards_delays: BackwardDelays,
    pub rtt_forward_delays: ForwardDelays,
    pub rtt_server_delays: ServerDelays,
}

/// An NTP source that varies over time based on pre-configured time series
///
/// This generator produces NTP events with time-varying properties (root delay,
/// root dispersion, network delays) defined by input time series. It automatically
/// handles interpolation to provide continuous values between defined data points.
///
/// The generator respects the input constraints of NTP exchanges and will stop
/// generating events once it reaches the end of the shortest input time series.
pub struct Generator {
    props: Props,
    id: String,
    next_poll: EstimateDuration,
    stop_after: TrueDuration,
}

#[bon::bon]
impl Generator {
    /// Creates a new time-varying NTP generator based on provided time series
    ///
    /// # Parameters
    /// * `poll_period` - Time between successive NTP requests
    /// * `id` - Identifier for the NTP source
    /// * `root_dispersions` - Time series of root dispersion values
    /// * `root_delays` - Time series of root delay values
    /// * `round_trip_delays` - Time series of network and server delay components
    ///
    /// # Returns
    /// A Result containing either the configured generator or an Error if any validation fails
    ///
    /// # Errors
    /// Returns an error if any time series doesn't meet requirements:
    /// - At least 2 data points
    /// - Must start at time 0
    /// - Must span at least one poll period
    #[builder]
    pub fn new(
        poll_period: EstimateDuration,
        id: String,
        root_dispersions: Series<TrueDuration, EstimateDuration>,
        root_delays: Series<TrueDuration, EstimateDuration>,
        round_trip_delays: &Series<TrueDuration, RoundTripDelays>,
    ) -> Result<Self, Error> {
        Generator::validate_poll_period(poll_period)?;
        Generator::validate_root_delays(&root_delays, poll_period)?;
        Generator::validate_root_dispersions(&root_dispersions, poll_period)?;
        Generator::validate_round_trip_delays(round_trip_delays, poll_period)?;

        let min_series_last_time = [
            *root_delays
                .indices()
                .last()
                .expect("Root delays series should not be empty."),
            *root_dispersions
                .indices()
                .last()
                .expect("Root dispersions series should not be empty."),
            *round_trip_delays
                .indices()
                .last()
                .expect("Round trip segment delays series should not be empty."),
        ]
        .into_iter()
        .min()
        .unwrap();

        let (rtt_backwards_delays, rtt_forward_delays, rtt_server_delays) =
            Generator::destructure_round_trip_delays(round_trip_delays);

        let props: Props = Props {
            poll_period,
            root_delays,
            root_dispersions,
            rtt_backwards_delays,
            rtt_forward_delays,
            rtt_server_delays,
        };

        Ok(Self {
            props,
            id,
            next_poll: poll_period,
            stop_after: min_series_last_time,
        })
    }

    /// Calculates all timestamps for an NTP event based on the current state
    ///
    /// Returns the client send/receive times and server receive/send times for
    /// the next NTP event, using the oscillator model to account for clock skew.
    ///
    /// # Returns
    /// `Some(NtpEventTimestamps)` if the next event is within bounds, `None` if no more events
    pub fn calculate_ntp_event_timestamps(
        &self,
        oscillator: &FullModel,
    ) -> Option<NtpEventTimestamps> {
        self.get_next_round_trip_delays(oscillator)?
            .calculate_ntp_event_timestamps(self.next_poll, oscillator)
    }

    /// Determines if another NTP event can be generated within the time series bounds
    ///
    /// # Returns
    /// `true` if the next poll time is within the bounds of all time series, `false` otherwise
    fn has_next(&self, oscillator_model: &FullModel) -> bool {
        oscillator_model
            .oscillator_estimate_to_true_duration(self.next_poll)
            .is_some_and(|true_next_poll| true_next_poll <= self.stop_after)
    }

    /// Gets the round trip delay components for the next NTP event
    ///
    /// Retrieves forward, backward and server delay values for the next poll time
    /// by interpolating from the configured time series.
    ///
    /// # Returns
    /// `Some(RoundTripDelays)` if the next event is within bounds, `None` if no more events
    fn get_next_round_trip_delays(&self, oscillator_model: &FullModel) -> Option<RoundTripDelays> {
        if !self.has_next(oscillator_model) {
            return None;
        }
        let true_next_poll =
            oscillator_model.oscillator_estimate_to_true_duration(self.next_poll)?;
        Some(RoundTripDelays {
            forward_network: self
                .props
                .rtt_forward_delays
                .approximate(true_next_poll)
                .unwrap_or_else(|| panic!(
                    "There should be a forward delay to approximate, given the poll time {:?} is no later than the last round trip delays series time {:?}.",
                    true_next_poll, self.props.rtt_forward_delays.indices().last()
                )),
            backward_network: self
                .props
                .rtt_backwards_delays
                .approximate(true_next_poll)
                .unwrap_or_else(|| panic!(
                    "There should be a backward delay to approximate, given the poll time {:?} is no later than the last round trip delays series time {:?}.",
                    true_next_poll, self.props.rtt_backwards_delays.indices().last()
                )),
            server: self
                .props
                .rtt_server_delays
                .approximate(true_next_poll)
                .unwrap_or_else(|| panic!(
                    "There should be a server delay to approximate, given the poll time {:?} is no later than the last round trip delays series time {:?}.",
                    true_next_poll, self.props.rtt_server_delays.indices().last()
                )),
        })
    }

    /// Validates that root dispersion series meets requirements
    ///
    /// # Errors
    /// Returns an error if the series:
    /// - Has fewer than 2 data points
    /// - Doesn't start at time 0
    /// - Doesn't span at least one poll period
    fn validate_root_dispersions(
        root_dispersions: &Series<TrueDuration, EstimateDuration>,
        poll_period: EstimateDuration,
    ) -> Result<(), Error> {
        if root_dispersions.len() < 2 {
            return Err(Error::TooFewDataPoints {
                data_series: "root_dispersions",
                min: 2,
                actual: root_dispersions.len(),
            });
        }

        if root_dispersions
            .indices()
            .first()
            .is_none_or(|first| *first != TrueDuration::from_secs(0))
        {
            return Err(Error::IllegalStartTime {
                data_series: "root_dispersions",
                required: TrueDuration::from_secs(0),
                first_data_tstamp: *root_dispersions.indices().first().unwrap(),
            });
        }

        if root_dispersions
            .indices()
            .last()
            .is_none_or(|last| *last < poll_period.assume_true())
        {
            return Err(Error::IllegalEndTime {
                data_series: "root_dispersions",
                poll_period,
                last_data_tstamp: *root_dispersions.indices().last().unwrap(),
            });
        }

        Ok(())
    }

    /// Validates that root delay series meets requirements
    ///
    /// # Errors
    /// Returns an error if the series:
    /// - Has fewer than 2 data points
    /// - Doesn't start at time 0
    /// - Doesn't span at least one poll period
    fn validate_root_delays(
        root_delays: &Series<TrueDuration, EstimateDuration>,
        poll_period: EstimateDuration,
    ) -> Result<(), Error> {
        if root_delays.len() < 2 {
            return Err(Error::TooFewDataPoints {
                data_series: "root_delays",
                min: 2,
                actual: root_delays.len(),
            });
        }

        if root_delays
            .indices()
            .first()
            .is_none_or(|first| *first != TrueDuration::from_secs(0))
        {
            return Err(Error::IllegalStartTime {
                data_series: "root_delays",
                required: TrueDuration::from_secs(0),
                first_data_tstamp: *root_delays.indices().first().unwrap(),
            });
        }

        if root_delays
            .indices()
            .last()
            .is_none_or(|last| *last < poll_period.assume_true())
        {
            return Err(Error::IllegalEndTime {
                data_series: "root_delays",
                poll_period,
                last_data_tstamp: *root_delays.indices().last().unwrap(),
            });
        }

        Ok(())
    }

    /// Validates that round trip delays series meets requirements
    ///
    /// # Errors
    /// Returns an error if the series:
    /// - Has fewer than 2 data points
    /// - Doesn't start at time 0
    /// - Doesn't span at least one poll period
    fn validate_round_trip_delays(
        round_trip_delays: &Series<TrueDuration, RoundTripDelays>,
        poll_period: EstimateDuration,
    ) -> Result<(), Error> {
        if round_trip_delays.len() < 2 {
            return Err(Error::TooFewDataPoints {
                data_series: "round_trip_delays",
                min: 2,
                actual: round_trip_delays.len(),
            });
        }
        if round_trip_delays
            .indices()
            .first()
            .is_none_or(|first| *first != TrueDuration::from_secs(0))
        {
            return Err(Error::IllegalStartTime {
                data_series: "round_trip_delays",
                required: TrueDuration::from_secs(0),
                first_data_tstamp: *round_trip_delays.indices().first().unwrap(),
            });
        }

        if round_trip_delays
            .indices()
            .last()
            .is_none_or(|last| *last < poll_period.assume_true())
        {
            return Err(Error::IllegalEndTime {
                data_series: "round_trip_delays",
                poll_period,
                last_data_tstamp: *round_trip_delays.indices().last().unwrap(),
            });
        }

        Ok(())
    }

    /// Validates the polling period
    ///
    /// # Errors
    /// Returns an error if the poll period is zero or negative
    fn validate_poll_period(poll_period: EstimateDuration) -> Result<(), Error> {
        if poll_period <= EstimateDuration::from_secs(0) {
            return Err(Error::IllegalPollPeriod {
                min: EstimateDuration::from_secs(1),
                actual: poll_period,
            });
        }
        Ok(())
    }

    /// Splits a round trip delays series into its component delay series
    ///
    /// Creates separate time series for forward network delays, backward network delays,
    /// and server processing delays to simplify interpolation for each component.
    ///
    /// # Returns
    /// A tuple of (`BackwardDelays`, `ForwardDelays`, `ServerDelays`) series
    fn destructure_round_trip_delays(
        round_trip_delays: &Series<TrueDuration, RoundTripDelays>,
    ) -> (BackwardDelays, ForwardDelays, ServerDelays) {
        // Create new series for each component
        let backward_delays = round_trip_delays
            .iter()
            .map(|(t, rtd)| (*t, rtd.backward_network))
            .collect();

        let forward_delays = round_trip_delays
            .iter()
            .map(|(t, rtd)| (*t, rtd.forward_network))
            .collect();

        let server_delays = round_trip_delays
            .iter()
            .map(|(t, rtd)| (*t, rtd.server))
            .collect();

        (
            BackwardDelays {
                inner: backward_delays,
            },
            ForwardDelays {
                inner: forward_delays,
            },
            ServerDelays {
                inner: server_delays,
            },
        )
    }
}

impl super::Generator for Generator {
    fn next_event_ready(&self, oscillator: &FullModel) -> Option<TscCount> {
        if !self.has_next(oscillator) {
            return None;
        }
        let ntp_event_tstamps = self.calculate_ntp_event_timestamps(oscillator)?;
        Some(ntp_event_tstamps.client_recv)
    }

    fn generate(&mut self, oscillator: &FullModel) -> Event {
        let NtpEventTimestamps {
            client_send,
            server_recv,
            server_send,
            client_recv,
        } = self
            .calculate_ntp_event_timestamps(oscillator)
            .expect("Next NTP event's timestamps should be computable based on the oscillator model if next_event_ready.");

        let true_next_poll = oscillator
            .oscillator_estimate_to_true_duration(self.next_poll)
            .expect("True next poll time should be computable based on the oscillator model estimate of the same if next_event_ready.");

        let root_delay = self
            .props
            .root_delays
            .approximate(true_next_poll)
            .unwrap_or_else(|| panic!(
                "Root delay at true next poll time {:?} should be computable, given poll time is no later than the generator's last root delays series time {:?}, if next_event_ready.",
                true_next_poll, self.props.root_delays.indices().last()
            ));

        let root_dispersion = self
            .props
            .root_dispersions
            .approximate(true_next_poll)
            .unwrap_or_else(|| panic!(
                "Root dispersion at true next poll time {:?} should be computable, given poll time is no later than the generator's last root dispersions series time {:?}, if next_event_ready.",
                true_next_poll, self.props.root_dispersions.indices().last()
            ));

        self.next_poll += self.props.poll_period;

        Event {
            variants: EventKind::Ntp(Ntp {
                server_system_recv_time: server_recv.demote_to_estimate(),
                server_system_send_time: server_send.demote_to_estimate(),
                root_delay,
                root_dispersion,
                source_id: self.id.clone(),
                client_system_times: None,
            }),
            client_tsc_pre_time: client_send,
            client_tsc_post_time: client_recv,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::time::{Frequency, Skew, TrueDuration, TrueInstant};
    use rstest::{fixture, rstest};

    use super::*;
    use crate::simulation::{ntp::Generator as _, oscillator::Oscillator};

    #[fixture]
    fn clock_freq_ghz() -> f64 {
        1.0
    }

    #[fixture]
    fn true_time_start_nanos() -> i128 {
        1_576_800_000_000_000_000 // 365 * 50 * 24 * 60 * 60 * 1000 * 1000 * 1000
    }

    #[fixture]
    fn oscillator_skew_ppm() -> f64 {
        -10.0
    }

    #[fixture]
    fn tsc_timestamp_client_start() -> i128 {
        10_000
    }

    #[fixture]
    fn starting_oscillator_offset_nanos() -> i128 {
        200_000
    }

    #[fixture]
    fn constant_skew_oscillator_model(
        clock_freq_ghz: f64,
        true_time_start_nanos: i128,
        oscillator_skew_ppm: f64,
        tsc_timestamp_client_start: i128,
        starting_oscillator_offset_nanos: i128,
    ) -> FullModel {
        let oscillator = Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(clock_freq_ghz))
            .start_time(TrueInstant::from_nanos(true_time_start_nanos))
            .duration(TrueDuration::from_hours(5))
            .tsc_timestamp_start(TscCount::new(tsc_timestamp_client_start))
            .skew(Skew::from_ppm(oscillator_skew_ppm))
            .starting_oscillator_offset(TrueDuration::from_nanos(starting_oscillator_offset_nanos))
            .call();
        FullModel::calculate_from_oscillator(oscillator)
    }

    #[fixture]
    fn poll_period() -> i128 {
        16
    }

    #[fixture]
    fn generator(poll_period: i128) -> Generator {
        let (root_delays, root_dispersions, rtds) = valid_series();
        Generator::builder()
            .poll_period(EstimateDuration::from_secs(poll_period))
            .id(String::from("series_test"))
            .root_delays(root_delays)
            .root_dispersions(root_dispersions)
            .round_trip_delays(&rtds)
            .build()
            .unwrap()
    }

    #[fixture]
    fn valid_series() -> (
        Series<TrueDuration, EstimateDuration>,
        Series<TrueDuration, EstimateDuration>,
        Series<TrueDuration, RoundTripDelays>,
    ) {
        let root_delays_data = [
            // (timestamp_secs, micros)
            (0, 100),
            (32, 150),
            (96, 200),
        ];
        let root_dispersions_data = [
            // (timestamp_secs, micros)
            (0, 200),
            (32, 250),
            (96, 300),
        ];
        let rtt_delays_data = [
            // (timestamp_secs, forward_μs, backward_μs, server_μs)
            (0, 50, 50, 40),
            (16, 80, 40, 60),
            (32, 50, 70, 40),
            (33, 50, 70, 40),
        ];

        let root_delays = root_delays_data
            .iter()
            .map(|(secs, micros)| {
                (
                    TrueDuration::from_secs(*secs),
                    EstimateDuration::from_micros(*micros),
                )
            })
            .collect();

        let root_dispersions = root_dispersions_data
            .iter()
            .map(|(secs, micros)| {
                (
                    TrueDuration::from_secs(*secs),
                    EstimateDuration::from_micros(*micros),
                )
            })
            .collect();

        let rtt_delays = rtt_delays_data
            .iter()
            .map(|(secs, fwd, bwd, svr)| {
                (
                    TrueDuration::from_secs(*secs),
                    RoundTripDelays::builder()
                        .forward_network(TrueDuration::from_micros(*fwd))
                        .backward_network(TrueDuration::from_micros(*bwd))
                        .server(TrueDuration::from_micros(*svr))
                        .build(),
                )
            })
            .collect();

        (root_delays, root_dispersions, rtt_delays)
    }

    #[test]
    fn test_successful_initialization() {
        let (root_delays, root_dispersions, rtds) = valid_series();

        let generator_result = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("series_test"))
            .root_delays(root_delays.clone())
            .root_dispersions(root_dispersions.clone())
            .round_trip_delays(&rtds.clone())
            .build();

        assert!(generator_result.is_ok());
        let generator = generator_result.unwrap();
        assert_eq!(generator.id, String::from("series_test"));
        assert_eq!(generator.next_poll, EstimateDuration::from_secs(16));
        assert_eq!(generator.stop_after, TrueDuration::from_secs(33)); // Min of all last times
    }

    #[test]
    fn test_validation_too_few_data_points() {
        // Create series with only one point
        let root_delays: Series<TrueDuration, EstimateDuration> = vec![(
            TrueDuration::from_secs(0),
            EstimateDuration::from_micros(100),
        )]
        .into_iter()
        .collect();

        let (_, root_dispersions, rtds) = valid_series();

        let result = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("series_test"))
            .root_delays(root_delays)
            .root_dispersions(root_dispersions)
            .round_trip_delays(&rtds)
            .build();

        assert!(result.is_err());
        match result {
            Err(Error::TooFewDataPoints {
                data_series,
                min,
                actual,
            }) => {
                assert_eq!(data_series, "root_delays");
                assert_eq!(min, 2);
                assert_eq!(actual, 1);
            }
            _ => panic!("Expected TooFewDataPoints error"),
        }
    }

    #[test]
    fn test_validation_illegal_start_time() {
        // Create series with non-zero start time
        let root_delays: Series<TrueDuration, EstimateDuration> = vec![
            (
                TrueDuration::from_secs(5),
                EstimateDuration::from_micros(100),
            ),
            (
                TrueDuration::from_secs(50),
                EstimateDuration::from_micros(150),
            ),
        ]
        .into_iter()
        .collect();

        let (_, root_dispersions, rtt_delays) = valid_series();

        let result = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("series_test"))
            .root_delays(root_delays)
            .root_dispersions(root_dispersions)
            .round_trip_delays(&rtt_delays)
            .build();

        assert!(result.is_err());
        match result {
            Err(Error::IllegalStartTime {
                data_series,
                required,
                first_data_tstamp,
            }) => {
                assert_eq!(data_series, "root_delays");
                assert_eq!(required, TrueDuration::from_secs(0));
                assert_eq!(first_data_tstamp, TrueDuration::from_secs(5));
            }
            _ => panic!("Expected IllegalStartTime error"),
        }
    }

    #[test]
    fn test_validation_illegal_end_time() {
        // Create series that doesn't span a full poll period
        let rtt_delays: Series<TrueDuration, RoundTripDelays> = vec![
            (
                TrueDuration::from_secs(0),
                RoundTripDelays::builder()
                    .forward_network(TrueDuration::from_micros(55))
                    .backward_network(TrueDuration::from_micros(67))
                    .server(TrueDuration::from_micros(35))
                    .build(),
            ),
            (
                TrueDuration::from_secs(10),
                RoundTripDelays::builder()
                    .forward_network(TrueDuration::from_micros(60))
                    .backward_network(TrueDuration::from_micros(70))
                    .server(TrueDuration::from_micros(40))
                    .build(),
            ),
        ]
        .into_iter()
        .collect();

        let (root_delays, root_dispersions, _) = valid_series();

        let result = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("series_test"))
            .root_delays(root_delays)
            .root_dispersions(root_dispersions)
            .round_trip_delays(&rtt_delays)
            .build();

        assert!(result.is_err());
        match result {
            Err(Error::IllegalEndTime {
                data_series,
                poll_period,
                last_data_tstamp,
            }) => {
                assert_eq!(data_series, "round_trip_delays");
                assert_eq!(poll_period, EstimateDuration::from_secs(16));
                assert_eq!(last_data_tstamp, TrueDuration::from_secs(10));
            }
            _ => panic!("Expected IllegalEndTime error"),
        }
    }

    #[test]
    fn test_validation_illegal_poll_period() {
        let (root_delays, root_dispersions, rtds) = valid_series();

        let result = Generator::builder()
            .poll_period(EstimateDuration::from_secs(0))
            .id(String::from("series_test"))
            .root_delays(root_delays)
            .root_dispersions(root_dispersions)
            .round_trip_delays(&rtds)
            .build();

        assert!(result.is_err());
        match result {
            Err(Error::IllegalPollPeriod { min, actual }) => {
                assert_eq!(min, EstimateDuration::from_secs(1));
                assert_eq!(actual, EstimateDuration::from_secs(0));
            }
            _ => panic!("Expected IllegalPollPeriod error"),
        }
    }

    #[rstest]
    fn test_next_event_ready(constant_skew_oscillator_model: FullModel, mut generator: Generator) {
        let next_event_ready = generator.next_event_ready(&constant_skew_oscillator_model);
        assert!(next_event_ready.is_some());
        let _ = generator.generate(&constant_skew_oscillator_model);
    }

    #[rstest]
    fn test_generate(constant_skew_oscillator_model: FullModel, mut generator: Generator) {
        let event = generator.generate(&constant_skew_oscillator_model);

        // Check source ID
        let ntp = event.variants.ntp().unwrap();
        assert_eq!(ntp.source_id, "series_test");

        // Check event type and timestamps
        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let client_start_timestamp_in_nanos =
            (tsc_timestamp_client_start() as f64 / clock_freq_ghz()) as i128;
        assert_eq!(
            event.client_tsc_pre_time,
            TscCount::new(poll_period() * 1_000_000_000 + client_start_timestamp_in_nanos)
        );

        let client_start_instant = poll_period() * 1_000_000_000 + client_start_timestamp_in_nanos;
        let client_start_instant = TrueInstant::from_nanos(client_start_instant);

        let expected = client_start_instant
            + generator
                .props
                .rtt_forward_delays
                .approximate(TrueDuration::from_secs(poll_period()))
                .unwrap()
            + generator
                .props
                .rtt_server_delays
                .approximate(TrueDuration::from_secs(poll_period()))
                .unwrap()
            + generator
                .props
                .rtt_backwards_delays
                .approximate(TrueDuration::from_secs(poll_period()))
                .unwrap();

        approx::assert_abs_diff_eq!(
            event.client_tsc_post_time.get() as i64,
            expected.as_nanos() as i64,
            epsilon = 2 // 2 ns tolerance
        );

        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let client_skew_between_polls_nanos =
            poll_period() as f64 * -oscillator_skew_ppm() * 1_000.0;

        let expected = constant_skew_oscillator_model.oscillator().start_time()
            + TrueDuration::from_nanos(poll_period() * 1_000_000_000)
            - TrueDuration::from_nanos(starting_oscillator_offset_nanos())
            + TrueDuration::from_seconds_f64(client_skew_between_polls_nanos / 1e9)
            + generator
                .props
                .rtt_forward_delays
                .approximate(TrueDuration::from_secs(poll_period()))
                .unwrap();

        approx::assert_abs_diff_eq!(
            ntp.server_system_recv_time.as_nanos() as i64,
            expected.as_nanos() as i64,
            epsilon = 1,
        );

        let expected = ntp.server_system_recv_time.assume_true()
            + generator
                .props
                .rtt_server_delays
                .approximate(TrueDuration::from_secs(poll_period()))
                .unwrap();

        approx::assert_abs_diff_eq!(
            ntp.server_system_send_time.as_nanos() as i64,
            expected.as_nanos() as i64,
            epsilon = 1,
        );

        let expected_root_delay = EstimateDuration::from_micros(125);
        let root_delay_delta = (ntp.root_delay.as_nanos() - expected_root_delay.as_nanos()).abs();
        assert!(
            root_delay_delta <= 1,
            "Root delay difference from expected is {:?} ns, expected <= 1 ns. Actual: {:?}, Expected: {:?}",
            root_delay_delta,
            ntp.root_delay,
            expected_root_delay
        );

        let expected_root_dispersion = EstimateDuration::from_micros(225);
        let root_dispersion_delta =
            (ntp.root_dispersion.as_nanos() - expected_root_dispersion.as_nanos()).abs();
        assert!(
            root_dispersion_delta <= 1,
            "Root dispersion difference from expected is {:?} ns, expected <= 1 ns. Actual: {:?}, Expected: {:?}",
            root_dispersion_delta,
            ntp.root_dispersion,
            expected_root_dispersion
        );
    }

    #[rstest]
    fn test_second_generation(constant_skew_oscillator_model: FullModel, mut generator: Generator) {
        let first_event = generator.generate(&constant_skew_oscillator_model);
        let second_event = generator.generate(&constant_skew_oscillator_model);

        let first_ntp = first_event.variants.ntp().unwrap();
        let second_ntp = second_event.variants.ntp().unwrap();

        #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
        let first_client_send_timestamp_in_nanos =
            (first_event.client_tsc_pre_time.get() as f64 / clock_freq_ghz()) as i128;

        assert_eq!(
            second_event.client_tsc_pre_time,
            TscCount::new(poll_period() * 1_000_000_000 + first_client_send_timestamp_in_nanos)
        );

        let expected = TrueInstant::from_nanos(poll_period() * 1_000_000_000)
            + TrueDuration::from_nanos(first_client_send_timestamp_in_nanos)
            + generator
                .props
                .rtt_forward_delays
                .approximate(TrueDuration::from_secs(2 * poll_period()))
                .unwrap()
            + generator
                .props
                .rtt_server_delays
                .approximate(TrueDuration::from_secs(2 * poll_period()))
                .unwrap()
            + generator
                .props
                .rtt_backwards_delays
                .approximate(TrueDuration::from_secs(2 * poll_period()))
                .unwrap();

        // relies on 1GHz clock
        approx::assert_abs_diff_eq!(
            second_event.client_tsc_post_time.get() as i64,
            expected.as_nanos() as i64,
            epsilon = 2 // 2 ns tolerance
        );

        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let client_skew_between_polls_nanos =
            (poll_period() as f64 * -oscillator_skew_ppm() * 1_000.0) as i128;

        let expected = first_ntp.server_system_recv_time.assume_true()
            - generator
                .props
                .rtt_forward_delays
                .approximate(TrueDuration::from_secs(poll_period()))
                .unwrap()
            + TrueDuration::from_nanos(poll_period() * 1_000_000_000)
            + TrueDuration::from_nanos(client_skew_between_polls_nanos)
            + generator
                .props
                .rtt_forward_delays
                .approximate(TrueDuration::from_secs(2 * poll_period()))
                .unwrap();

        approx::assert_abs_diff_eq!(
            second_ntp.server_system_recv_time.as_nanos() as i64,
            expected.as_nanos() as i64,
            epsilon = 1,
        );

        assert_eq!(
            second_ntp.server_system_send_time.as_nanos(),
            second_ntp.server_system_recv_time.as_nanos()
                + generator
                    .props
                    .rtt_server_delays
                    .approximate(TrueDuration::from_secs(2 * poll_period()))
                    .unwrap()
                    .as_nanos()
        );

        let expected_root_delay = EstimateDuration::from_micros(150);
        let root_delay_delta =
            (second_ntp.root_delay.as_nanos() - expected_root_delay.as_nanos()).abs();
        assert!(
            root_delay_delta <= 1,
            "Root delay difference from expected is {:?} ns, expected <= 1 ns. Actual: {:?}, Expected: {:?}",
            root_delay_delta,
            second_ntp.root_delay,
            expected_root_delay
        );

        let expected_root_dispersion = EstimateDuration::from_micros(250);
        let root_dispersion_delta =
            (second_ntp.root_dispersion.as_nanos() - expected_root_dispersion.as_nanos()).abs();
        assert!(
            root_dispersion_delta <= 1,
            "Root dispersion difference from expected is {:?} ns, expected <= 1 ns. Actual: {:?}, Expected: {:?}",
            root_dispersion_delta,
            second_ntp.root_dispersion,
            expected_root_dispersion
        );
    }

    #[rstest]
    fn test_has_next(constant_skew_oscillator_model: FullModel) {
        // Create series that end at different times
        let root_delays: Series<TrueDuration, EstimateDuration> = vec![
            (
                TrueDuration::from_secs(0),
                EstimateDuration::from_micros(100),
            ),
            (
                TrueDuration::from_secs(30),
                EstimateDuration::from_micros(150),
            ),
        ]
        .into_iter()
        .collect();

        let root_dispersions: Series<TrueDuration, EstimateDuration> = vec![
            (
                TrueDuration::from_secs(0),
                EstimateDuration::from_micros(200),
            ),
            (
                TrueDuration::from_secs(30),
                EstimateDuration::from_micros(250),
            ),
        ]
        .into_iter()
        .collect();

        let rtds: Series<TrueDuration, RoundTripDelays> = vec![
            (TrueDuration::from_secs(0), RoundTripDelays::default()),
            (TrueDuration::from_secs(30), RoundTripDelays::default()),
        ]
        .into_iter()
        .collect();

        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("test"))
            .root_delays(root_delays)
            .root_dispersions(root_dispersions)
            .round_trip_delays(&rtds)
            .build()
            .unwrap();

        // Should have events available at the beginning
        assert!(generator.has_next(&constant_skew_oscillator_model));

        // Create a generator that's already past its stop time
        let mut past_end_generator = generator;
        past_end_generator.next_poll = EstimateDuration::from_secs(32); // Beyond the 30s limit

        // Should no longer have events available
        assert!(!past_end_generator.has_next(&constant_skew_oscillator_model));
    }
}
