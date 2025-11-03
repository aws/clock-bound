//! Feed forward clock sync algorithm
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "remove when RoutableEvent is added")
)]

pub mod ff;

mod ring_buffer;

use std::net::SocketAddr;

pub use ring_buffer::RingBuffer;

use crate::daemon::{clock_parameters::ClockParameters, event};

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
#[derive(Debug, Clone, PartialEq, bon::Builder)]
pub struct ClockSyncAlgorithm {
    /// The link-local reference clock's ff algorithm
    link_local: source::LinkLocal,
    // A Vector of ff algorithms for ntp source reference clocks
    pub ntp_sources: Vec<source::NTPSource>,
}

impl ClockSyncAlgorithm {
    /// Feed event into the link local
    /// TODO: make this function private and call into it from `fn feed` when we have a routable event
    pub fn feed_link_local(&mut self, event: event::Ntp) -> Option<&ClockParameters> {
        self.link_local.feed(event)
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
        } = self;

        link_local.handle_disruption();
        for source in ntp_sources {
            source.handle_disruption();
        }
        tracing::info!("Handled clock disruption event");
    }
}
