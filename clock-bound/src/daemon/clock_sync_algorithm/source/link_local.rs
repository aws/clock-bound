//! Link local source

use std::num::NonZeroUsize;

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::ff::{self, event_buffer};
use crate::daemon::event;
use crate::daemon::time::Duration;

/// A Link Local reference clock source
///
/// Wraps around an NTP feed-forward clock-sync algorithm
#[derive(Debug, Clone)]
pub struct LinkLocal {
    inner: ff::Ntp,
}

impl LinkLocal {
    const POLL_INTERVAL: Duration = Duration::from_secs(2);

    // Poll every 2 seconds. Capacity is 1024 / 2 = 512
    const CAPACITY: NonZeroUsize = {
        let capacity =
            event_buffer::Local::<()>::SKM_WINDOW.as_seconds() / Self::POLL_INTERVAL.as_seconds();
        assert!(capacity > 0);
        #[expect(clippy::cast_sign_loss)]
        NonZeroUsize::new(capacity as usize).unwrap()
    };

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
