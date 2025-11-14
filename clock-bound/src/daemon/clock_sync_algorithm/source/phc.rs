//! PHC source

use std::path::PathBuf;

use crate::daemon::{
    clock_parameters::ClockParameters,
    clock_sync_algorithm::ff,
    event,
    time::{Duration, tsc::Skew},
};

/// A PHC from the Amazon Time Sync Service
///
/// Wraps around a PHC feed-forward clock sync algorithm
#[derive(Debug, Clone, PartialEq)]
pub struct Phc {
    device_path: PathBuf,
    inner: ff::Phc,
}

impl Phc {
    const POLL_INTERVAL: Duration = Duration::from_millis(500);

    /// Create a new PHC reference clock source
    pub fn new(device_path: PathBuf, max_dispersion: Skew) -> Self {
        Self {
            device_path,
            inner: ff::Phc::new(Self::POLL_INTERVAL, max_dispersion),
        }
    }

    /// Feed an event into the PHC clock sync algorithm
    #[tracing::instrument(level = "info", skip_all, fields(source = %self.device_path.display()))]
    pub fn feed(&mut self, event: event::Phc) -> Option<&ClockParameters> {
        self.inner.feed(event)
    }

    /// Handle a disruption event
    ///
    /// This clears the internal `ff` and any other state related to the local system's hardware
    pub fn handle_disruption(&mut self) {
        // Destructure pattern makes handling new fields mandatory
        let Self {
            device_path: _,
            inner,
        } = self;
        inner.handle_disruption();
    }
}
