//! NTP Source source

use std::net::SocketAddr;

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::ff;
use crate::daemon::event;
use crate::daemon::time::Duration;
use crate::daemon::time::tsc::Skew;

/// A NTP Server reference clock source
///
/// Wraps around an NTP feed-forward clock-sync algorithm.
/// May optionally belong to a DNS pool identified by `pool_domain`.
#[derive(Debug, Clone, PartialEq)]
pub struct NtpSource {
    source_address: SocketAddr,
    /// If this source was resolved from a DNS pool, the pool's domain name
    pool_domain: Option<String>,
    inner: ff::Ntp,
}

impl NtpSource {
    const POLL_INTERVAL: Duration = Duration::from_secs(16);

    /// Constructor
    pub fn new(
        source_address: SocketAddr,
        pool_domain: Option<String>,
        max_dispersion: Skew,
    ) -> Self {
        Self {
            source_address,
            pool_domain,
            inner: ff::Ntp::new(Self::POLL_INTERVAL, max_dispersion),
        }
    }

    /// Create a new NTP Source reference clock source
    pub fn new_ip_addr_source(source_address: SocketAddr, max_dispersion: Skew) -> Self {
        Self {
            source_address,
            pool_domain: None,
            inner: ff::Ntp::new(Self::POLL_INTERVAL, max_dispersion),
        }
    }

    /// Create a new NTP Source that belongs to a DNS pool
    pub fn new_with_pool(
        source_address: SocketAddr,
        pool_domain: String,
        max_dispersion: Skew,
    ) -> Self {
        Self {
            source_address,
            pool_domain: Some(pool_domain),
            inner: ff::Ntp::new(Self::POLL_INTERVAL, max_dispersion),
        }
    }

    pub fn socket_address(&self) -> SocketAddr {
        self.source_address
    }

    /// Get the pool domain this source belongs to, if any
    pub fn pool_domain(&self) -> Option<&str> {
        self.pool_domain.as_deref()
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
        let Self {
            inner,
            source_address: _,
            pool_domain: _,
        } = self;

        inner.handle_disruption();
    }
}
