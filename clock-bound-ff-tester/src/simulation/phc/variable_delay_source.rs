//! PHC event generator.
//!
//! ```text
//!                                            ┌────────┐                                     
//!                                            │phc_time│                                     
//!                                            └────────┘                                     
//! ───────────────────────────────────────────────┌┐┐─────────────────────────────────────────────►
//!                                              // │ \\\                                            
//!                                            //   │   \\                                           
//!                                          //     │     \\                                        
//!                                        //       │       \\                                     
//!                                      //         │         \\                                 
//!                                    //           │           \\                               
//!                                   /             │             \\                             
//!                                 //              │               \\                           
//!                               //                │                 \\                         
//!                              /                  │                   \\                        
//!                            //                   │                     \\\                      
//!                          //                     │                        \\\                    
//!                         /                       │                           \\                  
//!                       ┌┐                        │                             \┌┐                
//! ──────────────────────└┘────────────────────────┼──────────────────────────────└┘──────────────►
//!            ┌──────────────────┐                 │                      ┌────────────────┐         
//!            │ client_send_time │                 │                    ◄─│client_recv_time│─►         
//!            └────────┬─────────┘                 │                      └────────┬───────┘         
//!                     │                           │                               │                 
//!                     │                           │                               │                 
//!                     │                           │                               │                 
//!                     │                           │                               │                 
//!                     │◄─forward_network_delay───►│◄─◄─backward_network_delay────►│                 
//!                                                                                                            │                 
//! ```

use crate::events::v1::{Event, EventKind, Phc};
use crate::time::{DemoteToEstimate, EstimateDuration, TrueInstant, TscCount};

use crate::simulation::{delay::DelayRng, oscillator::FullModel};

use super::round_trip_delays::VariableRoundTripDelays;

use rand_chacha::{ChaCha12Rng, rand_core::SeedableRng};

/// Properties for [`Generator`]
#[derive(Debug)]
pub struct Props {
    /// The time period in which the oscillator polls the PHC
    ///
    /// An estimate, because this naively uses the local oscillator duration to drive
    /// when the next PHC request will start.
    ///
    /// In other words, if the local oscillator has a consistent offset, it will not account
    /// for that between PHC requests.
    ///
    /// Furthermore, if the clock has a skew, the next PHC request will not account for this,
    /// and instead use the uncorrected local time frame to drive the next PHC request.
    pub poll_period: EstimateDuration,
    /// The network channel parameters
    pub network_delays: VariableRoundTripDelays,
    /// The generated clock error bounds
    pub clock_error_bounds: Option<Box<dyn DelayRng>>,
}

/// Runs forever, like the juggernaut that it is
///
/// # Generation logic.
///
/// This struct continuously generates PHC events as long as the oscillator model will allow. It always aligns
/// with the start of the passed in oscillator. For example, if the oscillator is set to send an PHC every
/// 8 seconds (from the local oscillator's timeframe), it will start 8 seconds after the start of the passed in
/// `local_oscillator`.
///
/// Events are generated, driven by the local oscillator as well. The next poll corresponds to the `client_send_time`
/// in the diagram above. However, the value returned by [`Generator::next_event_ready`](crate::simulation::generator::Generator::next_event_ready)
/// corresponds to the `client_recv_time` in the diagram above.
#[derive(Debug)]
pub struct Generator {
    props: Props,
    id: String,
    next_poll: EstimateDuration,
    next_event: Option<PhcEventTimestamps>,
    rng: ChaCha12Rng,
}

#[bon::bon]
impl Generator {
    /// Construct with builder pattern
    #[builder]
    pub fn new(
        poll_period: EstimateDuration,
        id: String,
        oscillator: &FullModel,
        network_delays: VariableRoundTripDelays,
        clock_error_bounds: Option<Box<dyn DelayRng>>,
        seed: Option<u64>,
    ) -> Self {
        let props = Props {
            poll_period,
            network_delays,
            clock_error_bounds,
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
        let res = rv.calculate_phc_event_timestamps(oscillator);
        rv.next_event = res;
        rv
    }

    /// Return all of the PHC timestamps for an event given an oscillator model
    ///
    /// Useful if not using this struct as a generator, but as a primitive building block
    pub fn calculate_phc_event_timestamps(
        &mut self,
        oscillator: &FullModel,
    ) -> Option<PhcEventTimestamps> {
        // Generate the CEB
        let ceb = self.props.clock_error_bounds.as_mut().map(|ceb| {
            let ceb = ceb.get_value(&mut self.rng);
            ceb.demote_to_estimate()
        });
        self.props
            .network_delays
            .generate_round_trip_delays(&mut self.rng)
            .calculate_phc_event_timestamps(self.next_poll, oscillator, ceb)
    }
}

impl crate::simulation::generator::Generator for Generator {
    fn next_event_ready(&self, _oscillator: &FullModel) -> Option<TscCount> {
        self.next_event.clone().map(|v| v.client_recv)
    }

    fn generate(&mut self, oscillator: &FullModel) -> Event {
        let PhcEventTimestamps {
            client_send,
            phc_time,
            client_recv,
            clock_error_bound,
        } = {
            if self.next_event.is_some() {
                self.next_event.take().unwrap()
            } else {
                self.calculate_phc_event_timestamps(oscillator).unwrap()
            }
        };

        // update next poll time
        self.next_poll += self.props.poll_period;

        // Generate next event
        self.next_event = self.calculate_phc_event_timestamps(oscillator);

        Event {
            variants: EventKind::Phc(Phc {
                phc_time: phc_time.demote_to_estimate(),
                source_id: self.id.clone(),
                client_system_times: None,
                clock_error_bound,
            }),
            client_tsc_pre_time: client_send,
            client_tsc_post_time: client_recv,
        }
    }
}

/// Output of [`Generator::calculate_phc_event_timestamps`]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhcEventTimestamps {
    /// The tsc timestamp when the client sends the PHC request
    pub client_send: TscCount,
    /// The tsc timestamp of the PHC time
    pub phc_time: TrueInstant,
    /// The tsc timestamp when client receives the PHC request
    pub client_recv: TscCount,
    /// The Clock Error Bound, if configured
    pub clock_error_bound: Option<EstimateDuration>,
}

#[cfg(test)]
mod test {
    use crate::time::{EstimateDuration, Frequency, Skew, TrueDuration, TrueInstant, TscCount};
    use rstest::{fixture, rstest};

    use super::*;
    use crate::simulation::{
        delay::{Delay, TimeUnit},
        generator::Generator as _,
        oscillator::Oscillator,
        stats::DiracDistribution,
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
        let network_delays = VariableRoundTripDelays::test_default();
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("perfect_test"))
            .network_delays(network_delays)
            .oscillator(&constant_skew)
            .build();
        assert_eq!(generator.props.poll_period, EstimateDuration::from_secs(16));
        assert_eq!(generator.id, String::from("perfect_test"));
    }

    #[rstest]
    fn new(constant_skew: FullModel) {
        let forward_network_delay =
            Delay::new(DiracDistribution::new(1.0).unwrap(), TimeUnit::Micros);
        let backward_network_delay =
            Delay::new(DiracDistribution::new(1.0).unwrap(), TimeUnit::Micros);
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("asdf"))
            .network_delays(
                VariableRoundTripDelays::builder()
                    .forward_network(Box::new(forward_network_delay.clone()))
                    .backward_network(Box::new(backward_network_delay.clone()))
                    .build(),
            )
            .oscillator(&constant_skew)
            .build();
        assert_eq!(generator.props.poll_period, EstimateDuration::from_secs(16));
    }

    #[rstest]
    fn next_event_ready(constant_skew: FullModel) {
        let network_delays = VariableRoundTripDelays::test_default();
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("value"))
            .network_delays(network_delays)
            .oscillator(&constant_skew)
            .seed(0)
            .build();

        let next_event_ready = generator.next_event_ready(&constant_skew).unwrap();
        assert_eq!(next_event_ready, TscCount::new(16_000_010_079));
    }

    #[fixture]
    fn generator(constant_skew: FullModel) -> Generator {
        let variable_delay = VariableRoundTripDelays {
            forward_network: Box::new(Delay::new(
                DiracDistribution::new(1.0).unwrap(),
                TimeUnit::Micros,
            )),
            backward_network: Box::new(Delay::new(
                DiracDistribution::new(1.0).unwrap(),
                TimeUnit::Micros,
            )),
        };
        Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("value"))
            .network_delays(variable_delay)
            .oscillator(&constant_skew)
            .build()
    }

    #[fixture]
    fn ceb_generator(constant_skew: FullModel) -> Generator {
        let variable_delay = VariableRoundTripDelays {
            forward_network: Box::new(Delay::new(
                DiracDistribution::new(1.0).unwrap(),
                TimeUnit::Micros,
            )),
            backward_network: Box::new(Delay::new(
                DiracDistribution::new(1.0).unwrap(),
                TimeUnit::Micros,
            )),
        };

        let ceb: Box<dyn DelayRng> = Box::new(Delay::new(
            DiracDistribution::new(1.0).unwrap(),
            TimeUnit::Micros,
        ));

        Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("value"))
            .network_delays(variable_delay)
            .oscillator(&constant_skew)
            .clock_error_bounds(ceb)
            .build()
    }

    #[rstest]
    fn generate(constant_skew: FullModel, mut generator: Generator) {
        // this test tests against regression values.
        // If this test starts failing, evaluate whether or not this
        // is the best testing mechanism.
        let event = generator.generate(&constant_skew);
        assert_eq!(event.client_tsc_post_time, TscCount::new(16_000_012_000));
        let phc = event.variants.phc().unwrap();
        assert_eq!(event.client_tsc_pre_time, TscCount::new(16_000_010_000));
        assert_eq!(phc.phc_time.as_nanos(), 1_576_800_015_999_961_000);
    }

    #[rstest]
    fn generate_with_ceb(constant_skew: FullModel, mut ceb_generator: Generator) {
        let event = ceb_generator.generate(&constant_skew);
        let phc = event.variants.phc().unwrap();
        assert_eq!(
            phc.clock_error_bound,
            Some(EstimateDuration::from_micros(1))
        );
    }

    #[rstest]
    fn second_generation(constant_skew: FullModel, mut generator: Generator) {
        let _ = generator.generate(&constant_skew);
        let event = generator.generate(&constant_skew);
        assert_eq!(event.client_tsc_post_time, TscCount::new(32_000_012_000));
    }
}
