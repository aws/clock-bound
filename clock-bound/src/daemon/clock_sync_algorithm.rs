//! Feed forward clock sync algorithm
#![expect(dead_code, reason = "remove when RoutableEvent is added")]

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
#[derive(Debug, Clone, bon::Builder)]
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
}
