//! Link local source

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::ff;
use crate::daemon::event;
use crate::daemon::time::Duration;
use crate::daemon::time::tsc::Skew;

/// A Link Local reference clock source
///
/// Wraps around an NTP feed-forward clock-sync algorithm
#[derive(Debug, Clone, PartialEq)]
pub struct LinkLocal {
    inner: ff::Ntp,
}

impl LinkLocal {
    const POLL_INTERVAL: Duration = Duration::from_secs(2);

    /// Create a new Link Local reference clock source
    pub fn new(max_dispersion: Skew) -> Self {
        Self {
            inner: ff::Ntp::new(Self::POLL_INTERVAL, max_dispersion),
        }
    }

    /// Feed an event into the link local NTP clock-sync algorithm
    #[tracing::instrument(level = "info", skip_all, fields(source = "link_local"))]
    pub fn feed(&mut self, event: event::Ntp) -> Option<&ClockParameters> {
        self.inner.feed(event)
    }

    /// Handle a disruption event.
    ///
    /// This clears the internal `ff` and any other state related to the local system's hardware
    pub fn handle_disruption(&mut self) {
        // Destructure pattern makes handling new fields mandatory
        let Self { inner } = self;

        inner.handle_disruption();
    }
}
