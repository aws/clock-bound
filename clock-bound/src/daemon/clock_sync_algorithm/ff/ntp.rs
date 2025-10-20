//! The NTP Feed-forward time synchronization algorithm

use std::num::NonZeroUsize;

use super::event_buffer;
use crate::daemon::{clock_parameters::ClockParameters, event, time::tsc::Period};

/// Feed forward time synchronization algorithm for a single NTP source
#[derive(Debug, Clone)]
pub struct Ntp {
    /// Events within the current SKM (within 1000 seconds)
    local: event_buffer::Local<event::Ntp>,
    /// Best RTT values of each SKM over the last week
    estimate: event_buffer::Estimate<event::Ntp>,
    /// Current calculation of [`ClockParameters`]
    clock_parameters: Option<ClockParameters>,
    /// Current TSC period estimate
    period_estimate: Option<Period>,
}

impl Ntp {
    /// Create a new feed forward time synchronization algorithm
    ///
    /// `local_capacity` should be the number of data-points to span an SKM window.
    /// For example, if the source is expected to sample once every second, the `local_capacity`
    /// should have a max value of 1000.
    pub fn new(_local_capacity: NonZeroUsize) -> Self {
        todo!()
        // Self {
        //     local: event_buffer::Local::new(local_capacity),
        //     estimate: event_buffer::Estimate::new(),
        //     clock_parameters: None,
        //     period_estimate: None,
        // }
    }

    /// Feed an event into this algorithm
    ///
    /// Returns [`Some`] if the event has improved this source's [`ClockParameters`].
    pub fn feed(&mut self, _event: event::Ntp) -> Option<ClockParameters> {
        todo!()
    }

    /// Get the current [`ClockParameters`]
    pub fn clock_parameters(&self) -> Option<&ClockParameters> {
        self.clock_parameters.as_ref()
    }
}
