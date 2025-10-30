//! Feed forward clock sync algorithm
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "remove when RoutableEvent is added")
)]

pub mod ff;

mod ring_buffer;
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
}

impl ClockSyncAlgorithm {
    /// Feed event into the link local
    /// TODO: make this function private and call into it from `fn feed` when we have a routable event
    pub fn feed_link_local(&mut self, event: event::Ntp) -> Option<ClockParameters> {
        self.link_local.feed(event)
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
        let Self { link_local } = self;

        link_local.handle_disruption();
        tracing::info!("Handled clock disruption event");
    }
}
