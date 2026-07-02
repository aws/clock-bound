//! A `perfect` ntp source. Runs indefinitely, and every value is without error
//!
//! Note this does NOT mean zero clock error bound
//!
//! ```text
//!                                          ┌──────────────┐               ┌──────────────┐                                     
//!                                          │server_rx_time│               │server_tx_time│                                     
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
//!              ┌──────────────────┐               │                            │                    ┌────────────────┐         
//!              │ client_send_time │               │                            │                    │client_recv_time│         
//!              └────────┬─────────┘               │                            │                    └────────┬───────┘         
//!                       │                         │                            │                             │                 
//!                       │                         │                            │                             │                 
//!                       │                         │                            │                             │                 
//!                       │                         │                            │                             │                 
//!                       │forward_network_delay───►│◄───server_delay───────────►│◄─backward_network_delay────►│                 
//!                                                                                                            │                 
//! ```

use crate::events::v1::{Event, EventKind, Ntp};
use crate::time::{DemoteToEstimate, EstimateDuration, TscCount};

use crate::simulation::oscillator::FullModel;

use super::round_trip_delays::{NtpEventTimestamps, RoundTripDelays};

/// Properties for [`Generator`]
#[derive(Debug, Clone)]
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
    /// The network channel parameters
    pub network_delays: RoundTripDelays,
}

/// Runs forever, like the juggernaut that it is
///
/// # Generation logic.
///
/// This struct continuously generates NTP events as long as the oscillator model will allow. It always aligns
/// with the start of the passed in oscillator. For example, if the oscillator is set to send an NTP packet every
/// 8 seconds (from the local oscillator's timeframe), it will start 8 seconds after the start of the passed in
/// `local_oscillator`.
///
/// Events are generated, driven by the local oscillator as well. The next poll corresponds to the `client_send_time`
/// in the diagram above. However, the value returned by [`super::Generator::next_event_ready`] corresponds to the
/// `client_recv_time` in the diagram above.
pub struct Generator {
    props: Props,
    id: String,
    next_poll: EstimateDuration,
}

#[bon::bon]
impl Generator {
    /// Construct with builder pattern
    #[builder]
    pub fn new(
        poll_period: EstimateDuration,
        id: String,
        #[builder(default)] root_dispersion: EstimateDuration,
        #[builder(default)] root_delay: EstimateDuration,
        #[builder(default)] network_delays: RoundTripDelays,
    ) -> Self {
        let props = Props {
            root_dispersion,
            root_delay,
            poll_period,
            network_delays,
        };
        Self {
            props,
            id,
            next_poll: poll_period,
        }
    }

    /// Return all of the NTP timestamps for an event given an oscillator model
    ///
    /// Useful if not using this struct as a generator, but as a primitive building block
    pub fn calculate_ntp_event_timestamps(
        &self,
        oscillator: &FullModel,
    ) -> Option<NtpEventTimestamps> {
        self.props
            .network_delays
            .calculate_ntp_event_timestamps(self.next_poll, oscillator)
    }
}

impl super::Generator for Generator {
    fn next_event_ready(&self, oscillator: &FullModel) -> Option<TscCount> {
        let res = self.calculate_ntp_event_timestamps(oscillator)?;
        Some(res.client_recv)
    }

    fn generate(&mut self, oscillator: &FullModel) -> Event {
        let NtpEventTimestamps {
            client_send,
            server_recv,
            server_send,
            client_recv,
        } = self.calculate_ntp_event_timestamps(oscillator).unwrap();

        // update next poll time
        self.next_poll += self.props.poll_period;

        Event {
            variants: EventKind::Ntp(Ntp {
                server_system_recv_time: server_recv.demote_to_estimate(),
                server_system_send_time: server_send.demote_to_estimate(),
                root_delay: self.props.root_delay,
                root_dispersion: self.props.root_dispersion,
                client_system_times: None,
                source_id: self.id.clone(),
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

    #[test]
    fn new_defaults() {
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("perfect_test"))
            .build();
        assert_eq!(generator.props.root_dispersion, EstimateDuration::default());
        assert_eq!(generator.props.root_delay, EstimateDuration::default());
        assert_eq!(generator.props.poll_period, EstimateDuration::from_secs(16));
        assert_eq!(
            generator.props.network_delays.server,
            TrueDuration::default()
        );
        assert_eq!(generator.id, String::from("perfect_test"));
        assert_eq!(
            generator.props.network_delays.forward_network,
            TrueDuration::default()
        );
        assert_eq!(
            generator.props.network_delays.backward_network,
            TrueDuration::default()
        );
    }

    #[test]
    fn new() {
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("asdf"))
            .root_dispersion(EstimateDuration::new(1))
            .root_delay(EstimateDuration::new(2))
            .network_delays(
                RoundTripDelays::builder()
                    .server(TrueDuration::new(3))
                    .forward_network(TrueDuration::new(4))
                    .backward_network(TrueDuration::new(5))
                    .build(),
            )
            .build();
        assert_eq!(generator.props.root_dispersion, EstimateDuration::new(1));
        assert_eq!(generator.props.root_delay, EstimateDuration::new(2));
        assert_eq!(generator.props.poll_period, EstimateDuration::from_secs(16));
        assert_eq!(generator.props.network_delays.server, TrueDuration::new(3));
        assert_eq!(
            generator.props.network_delays.forward_network,
            TrueDuration::new(4)
        );
        assert_eq!(
            generator.props.network_delays.backward_network,
            TrueDuration::new(5)
        );
    }

    #[rstest]
    fn next_event_ready(constant_skew: FullModel) {
        let generator = Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("value"))
            .build();

        let next_event_ready = generator.next_event_ready(&constant_skew);
        assert_eq!(next_event_ready, Some(TscCount::new(16_000_010_000)));
    }

    #[fixture]
    fn generator() -> Generator {
        Generator::builder()
            .poll_period(EstimateDuration::from_secs(16))
            .id(String::from("value"))
            .root_delay(EstimateDuration::from_micros(120))
            .root_dispersion(EstimateDuration::from_micros(230))
            .network_delays(
                RoundTripDelays::builder()
                    .server(TrueDuration::from_micros(35))
                    .forward_network(TrueDuration::from_micros(55))
                    .backward_network(TrueDuration::from_micros(67))
                    .build(),
            )
            .build()
    }

    #[rstest]
    fn generate(constant_skew: FullModel, mut generator: Generator) {
        // this test tests against regression values.
        // If this test starts failing, evaluate whether or not this
        // is the best testing mechanism.
        let event = generator.generate(&constant_skew);
        assert_eq!(event.client_tsc_post_time, TscCount::new(16_000_166_999));
        let ntp = event.variants.ntp().unwrap();
        assert_eq!(event.client_tsc_pre_time, TscCount::new(16_000_010_000));
        assert_eq!(
            ntp.server_system_recv_time.as_nanos(),
            1_576_800_016_000_015_000,
        );
        assert_eq!(
            ntp.server_system_send_time.as_nanos(),
            1_576_800_016_000_050_000,
        );
        assert_eq!(ntp.root_delay, EstimateDuration::from_micros(120));
        assert_eq!(ntp.root_dispersion, EstimateDuration::from_micros(230));
    }

    #[rstest]
    fn second_generation(constant_skew: FullModel, mut generator: Generator) {
        let _ = generator.generate(&constant_skew);
        let event = generator.generate(&constant_skew);
        assert_eq!(event.client_tsc_post_time, TscCount::new(32_000_166_998));
    }
}
