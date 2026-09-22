//! Amazon Time Sync source

use std::net::{Ipv4Addr, SocketAddrV4};

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::FfSyncParams;
use crate::daemon::clock_sync_algorithm::ff;
use crate::daemon::event;
use crate::daemon::time::Duration;

/// Amazon Time Sync NTP endpoint address (Amazon Time Sync Service on EC2)
// FIXME: This duplicates the definition in daemon/io/ntp.rs. The algorithm should not
// depend on the daemon IO module, but both need this address. Consolidate once we refactor
// source identity to be passed in from the IO layer at construction time.
pub const AMAZON_TIME_SYNC_ADDRESS: SocketAddrV4 =
    SocketAddrV4::new(Ipv4Addr::new(169, 254, 169, 123), 123);

/// An Amazon Time Sync reference clock source
///
/// Wraps around an NTP feed-forward clock-sync algorithm
#[derive(Debug, Clone, PartialEq)]
pub struct AmazonTimeSync {
    source_address: SocketAddrV4,
    inner: ff::Ntp,
}

impl AmazonTimeSync {
    /// Expected sampling period; sizes the estimator's buffers (not a tunable).
    pub(super) const POLL_INTERVAL: Duration = Duration::from_secs(2);

    /// Create a new Amazon Time Sync reference clock source
    pub fn new(ff_sync_params: &FfSyncParams) -> Self {
        Self {
            source_address: AMAZON_TIME_SYNC_ADDRESS,
            inner: ff::Ntp::new(
                Self::POLL_INTERVAL,
                ff_sync_params,
                &ff_sync_params.amazon_time_sync,
            ),
        }
    }

    /// The source address of this Amazon Time Sync endpoint
    pub fn source_address(&self) -> SocketAddrV4 {
        self.source_address
    }

    /// Feed an event into the Amazon Time Sync NTP clock-sync algorithm
    #[tracing::instrument(level = "info", skip_all, fields(source = "amazon_time_sync"))]
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
