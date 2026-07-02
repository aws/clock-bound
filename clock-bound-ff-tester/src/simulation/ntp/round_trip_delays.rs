//! Building block types for round trip delay

use crate::time::{EstimateDuration, TrueDuration, TrueInstant, TscCount};

use crate::simulation::{
    delay::{Delay, DelayRng, TimeUnit},
    interpolation::SeriesInterpolation,
    oscillator::FullModel,
    stats,
};

impl Default for Delay<stats::DiracDistribution> {
    fn default() -> Self {
        Self::new(
            stats::DiracDistribution::new(0.0).unwrap(),
            TimeUnit::Millis,
        )
    }
}

/// Building block for taking a set of network delays and creating NTP events
#[derive(bon::Builder, Debug)]
pub struct VariableRoundTripDelays {
    /// the true time delay of the NTP request from the client to the server
    pub forward_network: Box<dyn DelayRng>,
    /// the true time delay of the NTP reply back from the server to the client
    pub backward_network: Box<dyn DelayRng>,
    /// The true time processing time of the NTP server
    pub server: Box<dyn DelayRng>,
}

impl VariableRoundTripDelays {
    pub fn generate_round_trip_delays(
        &self,
        rng: &mut rand_chacha::ChaCha12Rng,
    ) -> RoundTripDelays {
        RoundTripDelays {
            forward_network: self.forward_network.get_value(rng),
            backward_network: self.backward_network.get_value(rng),
            server: self.server.get_value(rng),
        }
    }
}

impl Default for VariableRoundTripDelays {
    /// `VariableRoundTripDelays` defaults are determined via analysis for our ec2 fleet.
    ///
    /// ## TLDR:
    /// Forward, backward and server delay values are pulled from gamma distributions.
    /// The parameters for each as of 05/30 are:
    /// |                   | shape  |  scale  | loc  |
    /// |---                |---     |---      |---   |
    /// | `forward_network` | 0.176  | 0.101   | 2.38E-7  |
    /// | `backward_network`| 0.176  | 0.101   | 2.38E-7  |
    /// | `server`          | 0.0056 | 0.0158  | 2.38E-7  |
    ///
    /// This should be used when attempting to model plausible networking delay.
    fn default() -> Self {
        VariableRoundTripDelays::builder()
            .forward_network(Box::new(Delay::new(
                stats::GammaDistribution::new(0.176, 1.0 / 0.101, 2.38 * 10.0f64.powi(-7)).unwrap(),
                TimeUnit::Secs,
            )))
            .backward_network(Box::new(Delay::new(
                stats::GammaDistribution::new(0.176, 1.0 / 0.101, 2.38 * 10.0f64.powi(-7)).unwrap(),
                TimeUnit::Secs,
            )))
            .server(Box::new(Delay::new(
                stats::GammaDistribution::new(0.0056, 1.0 / 0.0158, 2.38 * 10.0f64.powi(-7))
                    .unwrap(),
                TimeUnit::Secs,
            )))
            .build()
    }
}

/// Building block for taking a set of network delays and creating NTP events
#[derive(Debug, Clone, bon::Builder, PartialEq, Default)]
pub struct RoundTripDelays {
    /// the true time delay of the NTP request from the client to the server
    pub forward_network: TrueDuration,
    /// the true time delay of the NTP reply back from the server to the client
    pub backward_network: TrueDuration,
    /// The true time processing time of the NTP server
    pub server: TrueDuration,
}

impl RoundTripDelays {
    /// Returns the `forward_network_delay`
    pub fn forward_network(&self) -> TrueDuration {
        self.forward_network
    }

    /// Returns the `backward_network_delay`
    pub fn backward_network(&self) -> TrueDuration {
        self.backward_network
    }

    /// Returns the `server_delay`
    pub fn server(&self) -> TrueDuration {
        self.server
    }

    /// Calculate the NTP event timestamps driven by the local oscillator timestamp
    ///
    /// - `oscillator_estimated_send_time` is the estimated time after the start of the oscillator model when the local client sends the NTP response.
    /// - `oscillator` is the model of the local hardware oscillator
    pub fn calculate_ntp_event_timestamps(
        &self,
        oscillator_estimated_send_delay: EstimateDuration,
        oscillator: &FullModel,
    ) -> Option<NtpEventTimestamps> {
        // This algorithm works well by converting between the TSC domain to the TrueTime domain and back again.
        //
        // While this may *seem* roundabout, it's a simple way to understand the problem. Convert your start time into the True Time domain,
        // do your operations, and then convert back.

        // First we need the tsc timestamp at the client send time
        let client_send = oscillator_estimated_send_delay
            * oscillator.oscillator().clock_frequency()
            + oscillator.oscillator().tsc_timestamp_start();

        // Then, convert this tsc timestamp into a true time with a reverse linear interpolation
        let client_send_true = oscillator
            .true_time_to_tsc_timestamps()
            .as_ref()
            .reverse_approximate(client_send)?;

        // then add network and server delays
        let server_recv = client_send_true + self.forward_network;
        let server_send = server_recv + self.server;
        let client_recv = server_send + self.backward_network;

        // then convert back to the tsc timestamp domain
        let client_recv = oscillator
            .true_time_to_tsc_timestamps()
            .as_ref()
            .approximate(client_recv)?;

        Some(NtpEventTimestamps {
            client_send,
            server_recv,
            server_send,
            client_recv,
        })
    }

    /// Returns the total network delay
    pub fn network(&self) -> TrueDuration {
        self.forward_network + self.backward_network
    }
}

/// Output of [`NetworkChannel::calculate_ntp_event_timestamps`]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtpEventTimestamps {
    /// The tsc timestamp that the client sends the NTP request
    pub client_send: TscCount,
    /// The true time that the server receives the NTP request
    pub server_recv: TrueInstant,
    /// The true time that the server sends the NTP response
    pub server_send: TrueInstant,
    /// The tsc timestamp that the client receives the NTP response
    pub client_recv: TscCount,
}

#[cfg(test)]
mod test {
    use crate::time::{AssumeTrue, Frequency, Skew};

    use rand_chacha;
    use rand_chacha::rand_core::SeedableRng;
    use rstest::{fixture, rstest};

    use crate::simulation::{
        oscillator::Oscillator, stats::GammaDistribution, stats::NormalDistribution,
    };

    use super::*;

    #[fixture]
    fn perfect_oscillator() -> FullModel {
        // A perfect oscillator with a 1GHz frequency is a perfect monotonic tsc clock (aka 1 tick vs nanosecond)
        // This makes math a lot easier for the basic case
        let oscillator = Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .duration(TrueDuration::from_hours(5))
            .call();
        FullModel::calculate_from_oscillator(oscillator)
    }

    #[fixture]
    fn positive_skew_oscillator() -> FullModel {
        // A positive skew oscillator with a 1GHz frequency is a perfect monotonic tsc clock (aka 1 tick vs nanosecond)
        // This makes math a lot easier for the basic case
        let oscillator = Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .duration(TrueDuration::from_hours(5))
            .skew(Skew::from_percent(5.0)) // wow now thats I call a bad oscillator
            .call();
        FullModel::calculate_from_oscillator(oscillator)
    }

    #[rstest]
    fn calculate_ntp_event_timestamps_perfect(perfect_oscillator: FullModel) {
        // expectations
        //
        // The oscillator has no offsets, so therefore the ntp event timestamps should be easy to calculate
        // This is doubly so since the oscillator under test has a 1GHz clock. Meaning that TSCs have
        // the same frequency as a 1 nanosecond clock in TrueTime and EstimateTime types
        let start_time = perfect_oscillator.oscillator().start_time();
        let forward_network = TrueDuration::from_micros(50);
        let backward_network = TrueDuration::from_micros(55);
        let client_start = EstimateDuration::from_millis(100);
        let server = TrueDuration::from_micros(27);

        let delays = RoundTripDelays {
            forward_network,
            backward_network,
            server,
        };

        let event = delays.calculate_ntp_event_timestamps(client_start, &perfect_oscillator);

        assert_eq!(
            event,
            Some(NtpEventTimestamps {
                client_send: TscCount::new(client_start.as_nanos()),
                server_recv: start_time + client_start.assume_true() + forward_network,
                server_send: start_time + client_start.assume_true() + forward_network + server,
                client_recv: TscCount::new(
                    (client_start.assume_true() + forward_network + server + backward_network)
                        .as_nanos()
                ),
            })
        );
    }

    #[rstest]
    fn calculate_ntp_event_timestamps_positive_skew(positive_skew_oscillator: FullModel) {
        // expectations
        //
        // The generator in this test has a 1GHz clock, which matches a 1 nanosecond interval in
        // `ff-tester` time types. This means that a skew should be easy to test with comparison operators
        let start_time = positive_skew_oscillator.oscillator().start_time();
        let forward_network = TrueDuration::from_micros(50);
        let backward_network = TrueDuration::from_micros(55);
        let client_start = EstimateDuration::from_millis(100);
        let server = TrueDuration::from_micros(27);

        let delays = RoundTripDelays {
            forward_network,
            backward_network,
            server,
        };

        let event = delays.calculate_ntp_event_timestamps(client_start, &positive_skew_oscillator);

        let timestamps = event.unwrap();

        // the client_send tsc timestamp and client_start estimate time should agree.
        assert_eq!(
            timestamps.client_send,
            TscCount::new(client_start.as_nanos())
        );

        // Because the oscillator is skewed fast, the server recv time represents true time.
        // This inequality shows that the true time is actually earlier than the normal calculations would expect
        assert!(timestamps.server_recv < start_time + client_start.assume_true() + forward_network);
        assert!(
            timestamps.server_send
                < start_time + client_start.assume_true() + forward_network + server
        );

        // The corollary to above: when the reply comes back, the local oscillator increased at a faster rate than the true time during the NTP exchange
        assert!(
            timestamps.client_recv
                > TscCount::new(
                    (client_start.assume_true() + forward_network + server + backward_network)
                        .as_nanos()
                )
        );
    }
    #[rstest]
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    fn validate_networkdelay_from_distributions() {
        // expectations
        //
        //This is intended to test the generation of a NTP packet from [`VariableRoundTripDelays`].
        //We expect for network delays in the packet to loosely align with the means of the gamma, normal and Dirac probability distributions.
        let gamma_shape = 1.0;
        let gamma_rate = 1.0;
        let forward_network_distribution =
            GammaDistribution::new(gamma_shape, gamma_rate, 0.0).unwrap();

        let normal_mean = 2.0;
        let normal_std = 0.5;
        let backward_network_distribution =
            NormalDistribution::new(normal_mean, normal_std).unwrap();
        let server_delay_tsc = 27;
        let server = TrueDuration::from_micros(server_delay_tsc);

        let variable_delay = VariableRoundTripDelays {
            forward_network: Box::new(Delay {
                distribution: forward_network_distribution,
                unit: TimeUnit::Micros,
            }),
            backward_network: Box::new(Delay {
                distribution: backward_network_distribution,
                unit: TimeUnit::Millis,
            }),
            // NOTE
            // If we want apply a constant value that doesn't change, as in there is no variation,
            // this is how we do it.
            server: Box::new(Delay {
                distribution: stats::DiracDistribution::new(server_delay_tsc as f64).unwrap(),
                unit: TimeUnit::Micros,
            }),
        };

        let mut rng = rand_chacha::ChaCha12Rng::from_seed(Default::default());
        let mut delay_values = vec![];
        for _ in 0..1000 {
            delay_values.push(variable_delay.generate_round_trip_delays(&mut rng));
        }

        let (mean_forward_network_delay, mean_backward_network_delay, mean_server) = {
            let mut rv_fnd = TrueDuration::from_secs(0);
            let mut rv_bnd = TrueDuration::from_secs(0);
            let mut rv_s = TrueDuration::from_secs(0);

            for it in &delay_values {
                rv_fnd += it.forward_network;
                rv_bnd += it.backward_network;
                rv_s += it.server;
            }

            #[allow(clippy::cast_possible_wrap)]
            let n = delay_values.len();
            (rv_fnd / n, rv_bnd / n, rv_s / n)
        };
        assert!(
            (mean_forward_network_delay
                - TrueDuration::from_micros((gamma_shape / gamma_rate) as i64))
            .abs()
                < TrueDuration::from_micros((gamma_shape / gamma_rate.powi(2)) as i64)
        );
        assert!(
            (mean_backward_network_delay - TrueDuration::from_millis(normal_mean as i64)).abs()
                < TrueDuration::from_micros(500)
        );
        assert_eq!(mean_server, server);
    }
}
