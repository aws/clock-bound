//! PHC source

use crate::daemon::{
    clock_parameters::ClockParameters, clock_sync_algorithm::FfSyncParams,
    clock_sync_algorithm::ff, event, time::Duration,
};

use super::DevicePath;

/// A PHC from the Amazon Time Sync Service
///
/// Wraps around a PHC feed-forward clock sync algorithm
#[derive(Debug, Clone, PartialEq)]
pub struct Phc {
    device_path: DevicePath,
    inner: ff::Phc,
}

impl Phc {
    /// Expected sampling period; sizes the estimator's buffers (not a tunable).
    pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(500);

    /// Create a new PHC reference clock source
    pub fn new(device_path: DevicePath, ff_sync_params: &FfSyncParams) -> Self {
        Self {
            device_path,
            inner: ff::Phc::new(Self::POLL_INTERVAL, ff_sync_params, &ff_sync_params.phc),
        }
    }

    /// Feed an event into the PHC clock sync algorithm
    #[tracing::instrument(level = "info", skip_all, fields(source = %self.device_path))]
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
