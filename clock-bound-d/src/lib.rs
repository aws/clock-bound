//! ClockBound Daemon
//!
//! This crate implements the ClockBound daemon

mod chrony_client;
mod clock_bound_runner;
mod clock_snapshot_poller;
mod clock_state_fsm;
mod clock_state_fsm_no_disruption;
mod phc_utils;
pub mod signal;

use std::path::Path;
use std::str::FromStr;
use std::sync::atomic;

#[cfg(any(test, feature = "test"))]
use crate::phc_utils::MockPhcWithSysfsErrorBound as PhcWithSysfsErrorBound;
#[cfg(not(any(test, feature = "test")))]
use crate::phc_utils::PhcWithSysfsErrorBound;
use clock_bound_shm::ShmWriter;
use clock_bound_vmclock::{shm::VMCLOCK_SHM_DEFAULT_PATH, shm_reader::VMClockShmReader};
use chrony_client::UnixDomainSocket;
use clock_bound_runner::ClockBoundRunner;
use clock_snapshot_poller::chronyd_snapshot_poller::ChronyDaemonSnapshotPoller;
use tracing::{debug, error};

pub use phc_utils::get_error_bound_sysfs_path;

// TODO: make this a parameter on the CLI?
pub const CLOCKBOUND_SHM_DEFAULT_PATH: &str = "/var/run/clockbound/shm0";

/// PhcInfo holds the refid of the PHC in chronyd (i.e. PHC0), and the
/// interface on which the PHC is enabled.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PhcInfo {
    pub refid: u32,
    pub sysfs_error_bound_path: std::path::PathBuf,
}

/// Boolean value that tracks whether a manually triggered disruption is pending and need to be
/// actioned.
pub static FORCE_DISRUPTION_PENDING: atomic::AtomicBool = atomic::AtomicBool::new(false);

/// Boolean value that can be toggled to signal periods of forced disruption vs. "normal" periods.
pub static FORCE_DISRUPTION_STATE: atomic::AtomicBool = atomic::AtomicBool::new(false);

/// The status of the system clock reported by chronyd
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ChronyClockStatus {
    /// The status of the clock is unknown.
    Unknown = 0,

    /// The clock is kept accurate by the synchronization daemon.
    Synchronized = 1,

    /// The clock is free running and not updated by the synchronization daemon.
    FreeRunning = 2,
}

impl From<u16> for ChronyClockStatus {
    // Chrony is signalling it is not synchronized by setting both bits in the Leap Indicator.
    fn from(value: u16) -> Self {
        match value {
            0..=2 => Self::Synchronized,
            3 => Self::FreeRunning,
            _ => Self::Unknown,
        }
    }
}

/// Enum of possible Clock Disruption States exposed by the daemon.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ClockDisruptionState {
    Unknown,
    Reliable,
    Disrupted,
}

/// Custom struct used for indicating a parsing error when parsing a
/// ClockErrorBoundSource or ClockDisruptionNotificationSource
/// from str.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ParseError;

/// Enum of possible input sources for obtaining the ClockErrorBound.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ClockErrorBoundSource {
    /// Chrony.
    Chrony,

    /// VMClock.
    VMClock,
}

/// Performs a case-insensitive conversion from str to enum ClockErrorBoundSource.
impl FromStr for ClockErrorBoundSource {
    type Err = ParseError;
    fn from_str(input: &str) -> Result<ClockErrorBoundSource, Self::Err> {
        match input.to_lowercase().as_str() {
            "chrony" => Ok(ClockErrorBoundSource::Chrony),
            "vmclock" => Ok(ClockErrorBoundSource::VMClock),
            _ => {
                error!("ClockErrorBoundSource '{:?}' is not supported", input);
                Err(ParseError)
            }
        }
    }
}

/// Helper for converting a string ref_id into a u32 for the chrony command protocol.
///
/// # Arguments
///
/// * `ref_id` - The ref_id as a string to be translated to a u32.
pub fn refid_to_u32(ref_id: &str) -> Result<u32, String> {
    let bytes = ref_id.bytes();
    if bytes.len() <= 4 && bytes.clone().all(|b| b.is_ascii()) {
        let bytes_as_u32: Vec<u32> = bytes.map(|val| val as u32).collect();
        Ok(bytes_as_u32
            .iter()
            .rev()
            .enumerate()
            .fold(0, |acc, (i, val)| acc | (val << (i * 8))))
    } else {
        Err(String::from(
            "The PHC reference ID supplied was not a 4 character ASCII string.",
        ))
    }
}

pub fn run(
    max_drift_ppb: u32,
    maybe_phc_info: Option<PhcInfo>,
    clock_error_bound_source: ClockErrorBoundSource,
    clock_disruption_support_enabled: bool,
) {
    // Create a writer to update the clock error bound shared memory segment
    let mut writer = match ShmWriter::new(Path::new(CLOCKBOUND_SHM_DEFAULT_PATH)) {
        Ok(writer) => {
            debug!("Created a new ShmWriter");
            writer
        }
        Err(e) => {
            error!(
                "Failed to create the SHM writer at {:?} {}",
                CLOCKBOUND_SHM_DEFAULT_PATH, e
            );
            panic!("Failed to create SHM writer");
        }
    };
    let clock_status_snapshot_poller = match clock_error_bound_source {
        ClockErrorBoundSource::Chrony => ChronyDaemonSnapshotPoller::new(
            Box::new(UnixDomainSocket::default()),
            maybe_phc_info.map(|phc_info| {
                PhcWithSysfsErrorBound::new(phc_info.sysfs_error_bound_path, phc_info.refid)
            }),
        ),
        ClockErrorBoundSource::VMClock => {
            unimplemented!("VMClock ClockErrorBoundSource is not yet implemented");
        }
    };
    let mut vmclock_shm_reader = if !clock_disruption_support_enabled {
        None
    } else {
        match VMClockShmReader::new(VMCLOCK_SHM_DEFAULT_PATH) {
            Ok(reader) => Some(reader),
            Err(e) => {
                panic!(
                    "VMClockPoller: Failed to create VMClockShmReader. Please check if path {:?} exists and is readable. {:?}",
                    VMCLOCK_SHM_DEFAULT_PATH, e
                );
            }
        }
    };

    let mut clock_bound_runner =
        ClockBoundRunner::new(clock_disruption_support_enabled, max_drift_ppb);
    clock_bound_runner.run(
        &mut vmclock_shm_reader,
        &mut writer,
        clock_status_snapshot_poller,
        UnixDomainSocket::default(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_str_to_clockerrorboundsource_conversion() {
        assert_eq!(
            ClockErrorBoundSource::from_str("chrony"),
            Ok(ClockErrorBoundSource::Chrony)
        );
        assert_eq!(
            ClockErrorBoundSource::from_str("Chrony"),
            Ok(ClockErrorBoundSource::Chrony)
        );
        assert_eq!(
            ClockErrorBoundSource::from_str("CHRONY"),
            Ok(ClockErrorBoundSource::Chrony)
        );
        assert_eq!(
            ClockErrorBoundSource::from_str("cHrOnY"),
            Ok(ClockErrorBoundSource::Chrony)
        );
        assert_eq!(
            ClockErrorBoundSource::from_str("vmclock"),
            Ok(ClockErrorBoundSource::VMClock)
        );
        assert_eq!(
            ClockErrorBoundSource::from_str("VMClock"),
            Ok(ClockErrorBoundSource::VMClock)
        );
        assert_eq!(
            ClockErrorBoundSource::from_str("VMCLOCK"),
            Ok(ClockErrorBoundSource::VMClock)
        );
        assert_eq!(
            ClockErrorBoundSource::from_str("vmClock"),
            Ok(ClockErrorBoundSource::VMClock)
        );
        assert!(ClockErrorBoundSource::from_str("other").is_err());
        assert!(ClockErrorBoundSource::from_str("None").is_err());
        assert!(ClockErrorBoundSource::from_str("null").is_err());
        assert!(ClockErrorBoundSource::from_str("").is_err());
    }

    #[test]
    fn test_refid_to_u32() {
        // Test error cases
        assert!(refid_to_u32("morethan4characters").is_err());
        let non_valid_ascii_str = "©";
        assert!(non_valid_ascii_str.len() <= 4);
        assert!(refid_to_u32(non_valid_ascii_str).is_err());

        // Test actual parsing is as expected
        // ASCII values: P = 80, H = 72, C = 67, 0 = 48
        assert_eq!(
            refid_to_u32("PHC0").unwrap(),
            80 << 24 | 72 << 16 | 67 << 8 | 48
        );
        assert_eq!(refid_to_u32("PHC").unwrap(), 80 << 16 | 72 << 8 | 67);
        assert_eq!(refid_to_u32("PH").unwrap(), 80 << 8 | 72);
        assert_eq!(refid_to_u32("P").unwrap(), 80);
    }
}
