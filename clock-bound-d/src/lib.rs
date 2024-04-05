// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only

//! ClockBound Daemon
//!
//! This crate implements the ClockBound daemon

pub mod channels;
mod chrony_poller;
mod shm_writer;
pub mod signal;
pub mod thread_manager;

use chrony_candm::reply::Tracking;

/// Type alias for i64 for error bound values retrieved from PHC sysfs interface.
type PhcErrorBound = i64;

/// PhcInfo holds the refid of the PHC in chronyd (i.e. PHC0), and the
/// interface on which the PHC is enabled.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PhcInfo {
    pub refid: u32,
    pub sysfs_error_bound_path: std::path::PathBuf,
}

// The set of unique channel ID for message passing between threads.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum ChannelId {
    // The main thread
    MainThread,

    // The thread in charge of periodically polling chrony for tracking data
    ClockErrorBoundPoller,

    // The thread listeing to message to write / update the clockbound shared memory segment.
    ShmWriter,
}

/// The type of messages exchanged between threads
///
/// The variant names loosely follow the convention that the name starts with the thread /
/// component that originates the message.
#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    // Chrony polling thread sends tracking data and clock error bound.
    ClockErrorBoundData((Tracking, PhcErrorBound, libc::timespec)),

    // Chrony polling thread signals it failed to reach out to chronyd (within the grace period).
    ChronyNotRespondingGracePeriod,

    // Chrony polling thread signals it failed to reach out to chronyd and grace period is expired.
    ChronyNotResponding,

    // Chrony polling thread signals that it failed to retrieve the PHC error bound when syncing to PHC (within the grace period).
    PhcErrorBoundRetrievalFailedGracePeriod,

    // Chrony polling thread signals that it failed to retrieve the PHC error bound when syncing to PHC.
    PhcErrorBoundRetrievalFailed,

    // A thread signalling it has terminated.
    ThreadTerminate(ChannelId),

    // A thread signalling it has panicked.
    ThreadPanic(ChannelId),

    // Stop all threads and processing.
    ThreadAbort,
}

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

/// Gets the PHC Error Bound sysfs file path given a network interface name.
///
/// # Arguments
///
/// * `interface` - The network interface to lookup the PHC error bound path for.
pub fn get_error_bound_sysfs_path(interface: &str) -> Result<std::path::PathBuf, String> {
    let pci_slot_name = get_pci_slot_name(interface)?;
    Ok(std::path::PathBuf::from(format!(
        "/sys/bus/pci/devices/{}/phc_error_bound",
        pci_slot_name
    )))
}

/// Gets the PCI slot name for a given network interface name.
///
/// # Arguments
///
/// * `interface` - The network interface to lookup the PCI slot name for.
#[cfg(not(test))]
fn get_pci_slot_name(interface: &str) -> Result<String, String> {
    use std::io::Read;
    let uevent_path = format!("/sys/class/net/{}/device/uevent", interface);
    let mut contents = String::new();
    match std::fs::File::open(&uevent_path) {
        Ok(mut f) => {
            if let Err(e) = f.read_to_string(&mut contents) {
                return Err(format!(
                    "Failed to read contents of uevent file {} to string: {}",
                    uevent_path, e
                ));
            }
        }
        Err(e) => {
            return Err(format!(
                "Failed to open uevent file {} for PHC network interface specified: {}",
                uevent_path, e
            ));
        }
    };
    Ok(contents
        .lines()
        .find_map(|line| line.strip_prefix("PCI_SLOT_NAME="))
        .ok_or(format!(
            "Failed to find PCI_SLOT_NAME for interface {}",
            interface
        ))?
        .to_string())
}

/// Test specific impl of get_pci_slot_name.
/// Using this so that we can mock this method that would normally
/// read into sysfs.
#[cfg(test)]
fn get_pci_slot_name(interface: &str) -> Result<String, String> {
    if interface == "return_error" {
        Err(format!(
            "Failed to find PCI_SLOT_NAME for interface {}",
            interface
        ))
    } else {
        Ok(interface.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_get_error_bound_sysfs_path() {
        assert!(get_error_bound_sysfs_path("return_error").is_err());
        assert_eq!(
            get_error_bound_sysfs_path("pci_slot_return_val").unwrap(),
            std::path::PathBuf::from(format!(
                "/sys/bus/pci/devices/{}/phc_error_bound",
                "pci_slot_return_val"
            ))
        );
    }
}
