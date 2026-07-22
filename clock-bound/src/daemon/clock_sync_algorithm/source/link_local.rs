//! Link local source

use std::net::{Ipv4Addr, SocketAddrV4};

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::ff;
use crate::daemon::event;
use crate::daemon::time::Duration;
use crate::daemon::time::tsc::Skew;

/// Link-local NTP endpoint address (Amazon Time Sync Service on EC2)
// FIXME: This duplicates the definition in daemon/io/ntp.rs. The algorithm should not
// depend on the daemon IO module, but both need this address. Consolidate once we refactor
// source identity to be passed in from the IO layer at construction time.
pub const LINK_LOCAL_ADDRESS: SocketAddrV4 =
    SocketAddrV4::new(Ipv4Addr::new(169, 254, 169, 123), 123);

/// A Link Local reference clock source
///
/// Wraps around an NTP feed-forward clock-sync algorithm
#[derive(Debug, Clone, PartialEq)]
pub struct LinkLocal {
    source_address: SocketAddrV4,
    inner: ff::Ntp,
}

impl LinkLocal {
    const POLL_INTERVAL: Duration = Duration::from_secs(2);

    /// Create a new Link Local reference clock source
    pub fn new(max_dispersion: Skew) -> Self {
        Self {
            source_address: LINK_LOCAL_ADDRESS,
            inner: ff::Ntp::new(Self::POLL_INTERVAL, max_dispersion),
        }
    }

    /// The source address of this link-local endpoint
    pub fn source_address(&self) -> SocketAddrV4 {
        self.source_address
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
        let Self {
            source_address: _,
            inner,
        } = self;

        inner.handle_disruption();
    }
}
