//! NTP Source source

use std::net::SocketAddr;
use std::num::NonZeroUsize;

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_sync_algorithm::ff::{self, event_buffer};
use crate::daemon::event;
use crate::daemon::io::ntp::AWS_TEMP_PUBLIC_TIME_ADDRESSES;
use crate::daemon::time::Duration;
use crate::daemon::time::tsc::Skew;

/// A NTP Server reference clock source
///
/// Wraps around an NTP feed-forward clock-sync algorithm
#[derive(Debug, Clone, PartialEq)]
pub struct NTPSource {
    source_address: SocketAddr,
    inner: ff::Ntp,
}

impl NTPSource {
    const POLL_INTERVAL: Duration = Duration::from_secs(16);

    // Poll every 16 seconds. Capacity is 1024 / 16 = 64
    const CAPACITY: NonZeroUsize = {
        let capacity =
            event_buffer::Local::<()>::SKM_WINDOW.as_seconds() / Self::POLL_INTERVAL.as_seconds();
        // Check on this >>>
        assert!(capacity > 0);
        #[expect(clippy::cast_sign_loss)]
        NonZeroUsize::new(capacity as usize).unwrap()
    };

    /// Create a new NTP Source reference clock source
    pub fn new(source_address: SocketAddr, max_dispersion: Skew) -> Self {
        Self {
            source_address,
            inner: ff::Ntp::new(Self::CAPACITY, max_dispersion),
        }
    }

    /// Create a vector of NTP time sources referencing our public time IPs for alpha
    pub fn create_time_aws_sources(max_dispersion: Skew) -> Vec<Self> {
        let mut sources: Vec<Self> = Vec::new();
        for address in AWS_TEMP_PUBLIC_TIME_ADDRESSES {
            sources.push(Self {
                source_address: address,
                inner: ff::Ntp::new(Self::CAPACITY, max_dispersion),
            });
        }
        sources
    }

    pub fn socket_address(&self) -> SocketAddr {
        self.source_address
    }

    /// Feed an event into the NTP Source clock-sync algorithm
    #[tracing::instrument(level = "info", skip_all)]
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
