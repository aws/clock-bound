//! Link local source

use std::num::NonZeroUsize;

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::ff;
use crate::daemon::event;

/// A Link Local reference clock source
///
/// Wraps around an NTP feed-forward clock-sync algorithm
#[derive(Debug, Clone)]
pub struct LinkLocal {
    inner: ff::Ntp,
}

impl LinkLocal {
    // Poll every 2 seconds. Capacity is 1000 / 2 = 500
    const CAPACITY: NonZeroUsize = NonZeroUsize::new(500).unwrap();

    /// Create a new Link Local reference clock source
    pub fn new() -> Self {
        Self {
            inner: ff::Ntp::new(Self::CAPACITY),
        }
    }

    /// Feed an event into the link local NTP clock-sync algorithm
    #[tracing::instrument(level = "info", skip_all)]
    pub fn feed(&mut self, event: event::Ntp) -> Option<ClockParameters> {
        self.inner.feed(event)
    }
}

impl Default for LinkLocal {
    fn default() -> Self {
        Self::new()
    }
}
