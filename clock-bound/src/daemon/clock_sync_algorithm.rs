//! Feed forward clock sync algorithm
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "remove when RoutableEvent is added")
)]

pub mod ff;

mod ring_buffer;

use std::{net::SocketAddr, sync::Arc};

pub use ring_buffer::RingBuffer;

use crate::daemon::{
    clock_parameters::ClockParameters, event, receiver_stream::RoutableEvent,
    selected_clock::SelectedClockSource, subscriber::PRIMER_TARGET,
};

pub mod source;

/// ClockBound's Clock Sync Algorithm
///
/// The ClockBound clock sync algorithm’s role is to consume, transform, and relay input
/// clock sources to answer the singular question:
/// > What time is it?
///
/// The [`ClockSyncAlgorithm`] is a [sans-io](https://sans-io.readthedocs.io/) component
/// that feeds on time synchronization events, and outputs the singular best estimate for
/// the time, TSC frequency, and their associated errors.
///
/// # Usage
/// TODO
#[derive(Debug, Clone, bon::Builder)]
pub struct ClockSyncAlgorithm {
    /// The link-local reference clock's ff algorithm
    link_local: source::LinkLocal,
    // A Vector of ff algorithms for ntp source reference clocks
    pub ntp_sources: Vec<source::NTPSource>,
    /// Shared reference to the current selected clock source
    selected_clock: Arc<SelectedClockSource>,
}

impl ClockSyncAlgorithm {
    /// Feed the clock sync algorithm with a time synchronization event
    pub fn feed(&mut self, routable_event: RoutableEvent) -> Option<&ClockParameters> {
        #[cfg(all(feature = "test-side-by-side", not(test)))]
        {
            use crate::daemon::event::TscRtt;
            let Some(system_clock) = routable_event.system_clock() else {
                return self.feed_inner(routable_event);
            };
            let system = system_clock.system_time;
            let system_tsc = system_clock.tsc;
            let tsc_rtt = routable_event.rtt();
            let retval = self.feed_inner(routable_event);
            if let Some(new_params) = &retval {
                let system_clock_tsc_age = system_tsc - new_params.tsc_count;
                let system_clock_age = system_clock_tsc_age * new_params.period;

                let comparable_time = new_params.time + system_clock_age;
                let offset_from_system_clock = comparable_time - system;
                let ntp_rtt = tsc_rtt * new_params.period;

                tracing::info!(
                    ?system_clock_tsc_age,
                    ?offset_from_system_clock,
                    ?ntp_rtt,
                    ?tsc_rtt
                );
            }
            retval
        }
        #[cfg(any(not(feature = "test-side-by-side"), test))]
        {
            self.feed_inner(routable_event)
        }
    }

    /// Convenience function to allow for easy instrumenting
    fn feed_inner(&mut self, routable_event: RoutableEvent) -> Option<&ClockParameters> {
        match routable_event {
            RoutableEvent::LinkLocal(event) => self.feed_link_local(event),
            RoutableEvent::NtpSource(sender_address, event) => {
                self.feed_ntp_source(sender_address, &event)
            }
            RoutableEvent::Phc(_data) => {
                todo!("Implement PHC IO event delivery")
            }
        }
    }

    /// Feed event into the link local
    /// TODO: make this function private and call into it from `fn feed` when we have a routable event
    #[expect(clippy::missing_panics_doc, reason = "serialization will not fail")]
    pub fn feed_link_local(&mut self, event: event::Ntp) -> Option<&ClockParameters> {
        let serialized = serde_json::to_string(&event).unwrap();

        let output = self.link_local.feed(event);

        tracing::info!(
            target: PRIMER_TARGET,
            event = serialized,
            output = serde_json::to_string(&output).unwrap(),
            "feed link local"
        );
        output
    }

    /// Feed event into ntp source
    /// TODO: make this function private and call into it from `fn feed` when we have a routable event
    pub fn feed_ntp_source(
        &mut self,
        sender_address: SocketAddr,
        event: &event::Ntp,
    ) -> Option<&ClockParameters> {
        let mut clock_parameters: Option<&ClockParameters> = None;
        for source in &mut self.ntp_sources {
            if source.socket_address() == sender_address {
                clock_parameters = source.feed(event.clone());
                break;
            }
        }
        clock_parameters
    }

    /// Handle a clock disruption event
    ///
    /// Call this function after the system detects a VMClock disruption event.
    ///
    /// It will go through and clear the state (like startup).
    pub fn handle_disruption(&mut self) {
        // Use the destructure pattern to get a mutable reference to each item.
        //
        // This makes it a compilation error if we add a new field this Self without handling it here
        let Self {
            link_local,
            ntp_sources,
            selected_clock,
        } = self;

        selected_clock.none();
        link_local.handle_disruption();
        for source in ntp_sources {
            source.handle_disruption();
        }
        tracing::info!("Handled clock disruption event");
    }
}

#[cfg(test)]
mod tests {
    use crate::daemon::{
        event::Stratum,
        time::{Duration, Instant, TscCount, tsc::Skew},
    };

    use super::*;

    #[test]
    #[tracing_test::traced_test]
    fn feed_link_local_event() {
        // Most logs are permeable to change. Make sure that we log a json event.

        let event = event::Ntp::builder()
            .tsc_pre(TscCount::new(500))
            .tsc_post(TscCount::new(1000))
            .ntp_data(event::NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(2),
                root_delay: Duration::from_micros(50),
                root_dispersion: Duration::from_millis(17),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        let mut csa = ClockSyncAlgorithm::builder()
            .link_local(source::LinkLocal::new(Skew::from_ppm(15.0)))
            .ntp_sources(vec![])
            .selected_clock(Arc::new(SelectedClockSource::default()))
            .build();

        let clock_parameters = csa.feed_link_local(event.clone());
        assert!(clock_parameters.is_none());

        let serialized_event = serde_json::to_string(&event).unwrap();
        let serialized_output = serde_json::to_string(&clock_parameters).unwrap();

        // tracing escapes quotes
        let serialized_event = serialized_event.replace("\"", r#"\""#);
        let serialized_output = serialized_output.replace("\"", r#"\""#);

        assert!(logs_contain(&serialized_event));
        assert!(logs_contain(&serialized_output));
    }
}
