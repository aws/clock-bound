//! A ntp source with variable network delay. Runs indefinitely, and every value has some error.
//!
//! ```text
//!                                          ┌──────────────┐               ┌──────────────┐                                     
//!                                        ◄─│server_rx_time│─►           ◄─│server_tx_time│─►                                     
//!                                          └──────────────┘               └──────────────┘                                     
//! ───────────────────────────────────────────────┌┐────────────────────────────┌┐─────────────────────────────────────────────►
//!                                              // │                            │\\\                                            
//!                                            //   │                            │  \\                                           
//!                                          //     │                            │    \\                                        
//!                                        //       │                            │      \\                                     
//!                                      //         │                            │        \\                                 
//!                                    //           │                            │          \\                               
//!                                   /             │                            │            \\                             
//!                                 //              │                            │              \\                           
//!                               //                │                            │                \\                         
//!                              /                  │                            │                  \\                        
//!                            //                   │                            │                    \\\                      
//!                          //                     │                            │                       \\\                    
//!                         /                       │                            │                          \\                  
//!                       ┌┐                        │                            │                            \┌┐                
//! ──────────────────────└┘────────────────────────┼────────────────────────────┼─────────────────────────────└┘──────────────►
//!            ┌──────────────────┐                 │                            │                    ┌────────────────┐         
//!            │ client_send_time │                 │                            │                  ◄─│client_recv_time│─►         
//!            └────────┬─────────┘                 │                            │                    └────────┬───────┘         
//!                     │                           │                            │                             │                 
//!                     │                           │                            │                             │                 
//!                     │                           │                            │                             │                 
//!                     │                           │                            │                             │                 
//!                     │◄─forward_network_delay───►│◄───server_delay───────────►│◄─backward_network_delay────►│                 
//!                                                                                                            │                 
//! ```

use crate::events::v1::{Event, EventKind, Ntp};
use crate::time::{DemoteToEstimate, EstimateDuration, TscCount};

use crate::simulation::oscillator::FullModel;

use super::round_trip_delays::{NtpEventTimestamps, VariableRoundTripDelays};

use rand_chacha::{ChaCha12Rng, rand_core::SeedableRng};

/// Properties for [`Generator`]
pub struct Props {
    /// The root dispersion that is always returned
    pub root_dispersion: EstimateDuration,
    /// The root delay in the NTP source that is always returned
    pub root_delay: EstimateDuration,
    /// The time period in which the oscillator polls the NTP server
    ///
    /// An estimate, because this naively uses the local oscillator duration to drive
    /// when the next NTP request will start.
    ///
    /// In other words, if the local oscillator has a consistent offset, it will not account
    /// for that between NTP requests.
    ///
    /// Furthermore, if the clock has a skew, the next NTP request will not account for this,
    /// and instead use the uncorrected local time frame to drive the next NTP request.
    pub poll_period: EstimateDuration,
    /// The network delay channel parameters. Each channel delay is defined by a probability
    /// distribution which values are pulled from.
    pub network_delays: VariableRoundTripDelays,
}

/// # Generation logic.
///
/// This struct continuously generates NTP events as long as the oscillator model will allow. It always aligns
/// with the start of the passed in oscillator. For example, if the oscillator is set to send an NTP packet every
/// 8 seconds (from the local oscillator's timeframe), it will start 8 seconds after the start of the passed in
/// `local_oscillator`.
///
/// Events are generated, driven by the local oscillator. The next poll corresponds to the `client_send_time`
/// in the diagram above. However, the value returned by [`super::Generator::next_event_ready`] corresponds to the
/// `client_recv_time` in the diagram above.
///
/// The `client_recv_time` is not consistent, unlike the `Perfect` generator. Each leg of the NTP
/// packet delay is pulled from a random distribution, provided by the caller.
pub struct Generator {
    props: Props,
    id: String,
    next_poll: EstimateDuration,
    next_event: Option<NtpEventTimestamps>,
    rng: ChaCha12Rng,
}

#[bon::bon]
impl Generator {
    /// Construct with builder pattern
    #[builder]
    pub fn new(
        poll_period: EstimateDuration,
        id: String,
        network_delays: VariableRoundTripDelays,
        oscillator: &FullModel,
        #[builder(default)] root_dispersion: EstimateDuration,
        #[builder(default)] root_delay: EstimateDuration,
        seed: Option<u64>,
    ) -> Self {
        let props = Props {
            root_dispersion,
            root_delay,
            poll_period,
            network_delays,
        };

        let rng = seed.map_or_else(
            || ChaCha12Rng::from_rng(rand::rngs::OsRng).unwrap(),
            ChaCha12Rng::seed_from_u64,
        );
        let mut rv = Self {
            props,
            id,
            next_poll: poll_period,
            next_event: None,
            rng,
        };

        // Generate the next event if possible.
        //
        // This prep work is needed to properly integrate with the
        // intended use case of the `Generator` trait.
        let res = rv.calculate_ntp_event_timestamps(oscillator);
        rv.next_event = res;
        rv
    }

    /// Return all of the NTP timestamps for an event given an oscillator model.
    ///
    /// Useful if not using this struct as a generator, but as a primitive building block.
    pub fn calculate_ntp_event_timestamps(
        &mut self,
        oscillator: &FullModel,
    ) -> Option<NtpEventTimestamps> {
        self.props
            .network_delays
            .generate_round_trip_delays(&mut self.rng)
            .calculate_ntp_event_timestamps(self.next_poll, oscillator)
    }
}

impl super::Generator for Generator {
    fn next_event_ready(&self, _oscillator: &FullModel) -> Option<TscCount> {
        self.next_event.clone().map(|v| v.client_recv)
    }

    fn generate(&mut self, oscillator: &FullModel) -> Event {
        let NtpEventTimestamps {
            client_send,
            server_recv,
            server_send,
            client_recv,
        } = {
            if self.next_event.is_some() {
                self.next_event.take().unwrap()
            } else {
                self.calculate_ntp_event_timestamps(oscillator).unwrap()
            }
        };

        // update next poll time
        self.next_poll += self.props.poll_period;

        // Generate next event
        self.next_event = self.calculate_ntp_event_timestamps(oscillator);

        Event {
            variants: EventKind::Ntp(Ntp {
                server_system_recv_time: server_recv.demote_to_estimate(),
                server_system_send_time: server_send.demote_to_estimate(),
                root_delay: self.props.root_delay,
                root_dispersion: self.props.root_dispersion,
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
    use crate::time::{EstimateInstant, Frequency, Skew, TrueDuration, TrueInstant};
    use rstest::{fixture, rstest};

    use super::*;
    use crate::simulation::{
        delay::{Delay, TimeUnit},
        ntp::Generator as _,
        oscillator::Oscillator,
        stats::GammaDistribution,
    };

    #[fixture]
    fn constant_skew() -> FullModel {
        let oscillator = Oscillator::create_simple()
            .clock_frequency(Frequency::from_ghz(1.0))
            .start_time(TrueInstant::from_days(365 * 50))
            .duration(TrueDuration::from_hours(5))
            .tsc_timestamp_start(TscCount::new(10_000))
            .skew(Skew::from_ppm(-10.0))
            .starting_oscillator_offset(TrueDuration::from_micros(200))
            .call();
        FullModel::calculate_from_oscillator(oscillator)
    }

    #[rstest]
    fn new_defaults(constant_skew: FullModel) {
        let network_delays = VariableRoundTripDelays::default();
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("perfect_test"))
            .network_delays(network_delays)
            .oscillator(&constant_skew)
            .build();
        assert_eq!(generator.props.root_dispersion, EstimateDuration::default());
        assert_eq!(generator.props.root_delay, EstimateDuration::default());
        assert_eq!(generator.props.poll_period, EstimateDuration::from_secs(16));
        assert_eq!(generator.id, String::from("perfect_test"));
    }

    #[rstest]
    fn new(constant_skew: FullModel) {
        let server_delay = Delay::new(
            GammaDistribution::new(1.0, 1.0, 35.0).unwrap(),
            TimeUnit::Micros,
        );
        let forward_network_delay = Delay::new(
            GammaDistribution::new(1.0, 1.0, 55.0).unwrap(),
            TimeUnit::Micros,
        );
        let backward_network_delay = Delay::new(
            GammaDistribution::new(1.0, 1.0, 67.0).unwrap(),
            TimeUnit::Micros,
        );
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("asdf"))
            .root_dispersion(EstimateDuration::new(1))
            .root_delay(EstimateDuration::new(2))
            .network_delays(
                VariableRoundTripDelays::builder()
                    .server(Box::new(server_delay.clone()))
                    .forward_network(Box::new(forward_network_delay.clone()))
                    .backward_network(Box::new(backward_network_delay.clone()))
                    .build(),
            )
            .oscillator(&constant_skew)
            .build();
        assert_eq!(generator.props.root_dispersion, EstimateDuration::new(1));
        assert_eq!(generator.props.root_delay, EstimateDuration::new(2));
        assert_eq!(generator.props.poll_period, EstimateDuration::from_secs(16));
    }

    #[rstest]
    fn next_event_ready(constant_skew: FullModel) {
        let network_delays = VariableRoundTripDelays::default();
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("value"))
            .network_delays(network_delays)
            .oscillator(&constant_skew)
            .seed(2)
            .build();

        let next_event_ready = generator.next_event_ready(&constant_skew).unwrap();
        assert!(next_event_ready >= TscCount::new(16_000_009_999));
    }

    #[fixture]
    fn generator(constant_skew: FullModel) -> Generator {
        let variable_delay = VariableRoundTripDelays {
            forward_network: Box::new(Delay::new(
                GammaDistribution::new(1.0, 1.0, 55.0).unwrap(),
                TimeUnit::Micros,
            )),
            backward_network: Box::new(Delay::new(
                GammaDistribution::new(1.0, 1.0, 67.0).unwrap(),
                TimeUnit::Micros,
            )),
            server: Box::new(Delay::new(
                GammaDistribution::new(1.0, 1.0, 35.0).unwrap(),
                TimeUnit::Micros,
            )),
        };
        Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("value"))
            .root_delay(EstimateDuration::from_micros(120))
            .root_dispersion(EstimateDuration::from_micros(230))
            .network_delays(variable_delay)
            .oscillator(&constant_skew)
            .build()
    }

    #[rstest]
    fn generate(constant_skew: FullModel, mut generator: Generator) {
        // this test tests against regression values.
        // If this test starts failing, evaluate whether or not this
        // is the best testing mechanism.
        let event = generator.generate(&constant_skew);
        assert!(event.client_tsc_post_time >= TscCount::new(16_000_166_997));
        let ntp = event.variants.ntp().unwrap();
        assert_eq!(event.client_tsc_pre_time, TscCount::new(16_000_010_000));
        assert!(
            ntp.server_system_recv_time >= EstimateInstant::from_nanos(1_576_800_016_000_014_999)
        );
        assert!(
            ntp.server_system_send_time >= EstimateInstant::from_nanos(1_576_800_016_000_049_999),
        );
        assert_eq!(ntp.root_delay, EstimateDuration::from_micros(120));
        assert_eq!(ntp.root_dispersion, EstimateDuration::from_micros(230));
    }

    #[rstest]
    fn second_generation(constant_skew: FullModel, mut generator: Generator) {
        let _ = generator.generate(&constant_skew);
        let event = generator.generate(&constant_skew);
        assert!(event.client_tsc_post_time >= TscCount::new(32_000_166_998));
    }
}
