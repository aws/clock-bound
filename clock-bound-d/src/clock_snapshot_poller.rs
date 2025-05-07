use nix::sys::time::TimeSpec;

use crate::ChronyClockStatus;

pub(crate) mod chronyd_snapshot_poller;

/// Trait for retrieving a snapshot of clock sync information
pub trait ClockStatusSnapshotPoller {
    fn retrieve_clock_status_snapshot(
        &self,
        as_of: TimeSpec,
    ) -> anyhow::Result<ClockStatusSnapshot>;
}

/// A snapshot of clock sync information at some particular time (CLOCK_MONOTONIC).
#[derive(Debug)]
pub struct ClockStatusSnapshot {
    pub error_bound_nsec: i64,
    pub chrony_clock_status: ChronyClockStatus,
    pub as_of: TimeSpec,
}
