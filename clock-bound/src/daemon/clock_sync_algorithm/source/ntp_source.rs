//! NTP Source source

use std::net::SocketAddr;

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::ff;
use crate::daemon::event;
use crate::daemon::time::Duration;
use crate::daemon::time::tsc::Skew;

/// A NTP Server reference clock source
///
/// Wraps around an NTP feed-forward clock-sync algorithm
#[derive(Debug, Clone, PartialEq)]
pub struct NtpSource {
    source_address: SocketAddr,
    inner: ff::Ntp,
}

impl NtpSource {
    const POLL_INTERVAL: Duration = Duration::from_secs(16);

    /// Create a new NTP Source reference clock source
    pub fn new(source_address: SocketAddr, max_dispersion: Skew) -> Self {
        Self {
            source_address,
            inner: ff::Ntp::new(Self::POLL_INTERVAL, max_dispersion),
        }
    }

    pub fn socket_address(&self) -> SocketAddr {
        self.source_address
    }

    /// Feed an event into the NTP Source clock-sync algorithm
    #[tracing::instrument(level = "info", skip_all, fields(source = %self.source_address))]
    pub fn feed(&mut self, event: event::Ntp) -> Option<&ClockParameters> {
        self.inner.feed(event)
    }

    /// Handle a disruption event.
    ///
    /// This clears the internal `ff` and any other state related to the local system's hardware
    pub fn handle_disruption(&mut self) {
        // Destructure pattern makes handling new fields mandatory
        let Self { inner, .. } = self;

        inner.handle_disruption();
    }
}
