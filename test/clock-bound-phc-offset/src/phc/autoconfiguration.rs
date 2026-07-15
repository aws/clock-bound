//! Most of this module is copy-paste from the daemon io/phc library.
//! Duplication here let's us sub in the slightly modified types we've setup in this testing library.

use std::{fs::File, io, path::PathBuf};

use thiserror::Error;
use tracing::{debug, warn};

use crate::phc::PhcReader;

#[derive(Debug, Error)]
pub enum PhcError {
    #[error("IO failure")]
    Io(#[from] io::Error),
    #[error("File does not exist")]
    FileNotFound(String),
    #[error("PTP device not found")]
    PtpDeviceNotFound(String),
    #[error("PTP device name not found")]
    PtpDeviceNameNotFound(String),
    #[error("PCI_SLOT_NAME not found in uevent file")]
    PciSlotNameNotFound(String),
    #[error("PHC clock error bound file not found for PCI slot name {pci_slot_name}")]
    PhcClockErrorBoundFileNotFound { pci_slot_name: String },
    #[error("Device driver name not found")]
    DeviceDriverNameNotFound(String),
    #[error("Unexpected error")]
    UnexpectedError(String),
}

/// Attempts to autoconfigure readers for the PHC and PHC clock error bound
/// by navigating and reading from the filesystem to obtain PTP device details.
///
/// If there are no eligible PTP devices found then a `PhcError::PtpDeviceNotFound`
/// will be returned in the Result.
///
/// # Errors
/// Returns an error if autoconfiguration fails.
#[expect(
    clippy::too_many_lines,
    reason = "This function is already refactored to call
    separate functions for specific functionality.  The big for loop is needed
    because we have many `continue` statements in the loop body, and other alternate
    approaches would make the code harder to follow than the current implementation."
)]
pub fn autoconfigure_phc_reader() -> Result<PhcReader, PhcError> {
    // Get the list of network interfaces.
    let network_interfaces = match get_network_interfaces() {
        Ok(network_interfaces) => network_interfaces,
        Err(e) => {
            warn!(
                error = ?e,
                "PHC reader autoconfiguration failed due to inability to get the list of network interfaces."
            );
            return Err(e);
        }
    };

    // Create a vec of tuples holding the PTP device path and PHC clock error bound sysfs path.
    // Each tuple entry in this vec is a valid PHC configuration.
    let mut ptp_device_path_and_phc_clock_error_bound_sysfs_path_vec: Vec<(String, String)> =
        Vec::new();

    for network_interface in network_interfaces {
        debug!(
            ?network_interface,
            "Gathering information on network_interface",
        );

        let uevent_file_path = match get_uevent_file_path_for_network_interface(&network_interface)
        {
            Ok(uevent_file_path) => {
                debug!(
                    ?network_interface,
                    ?uevent_file_path,
                    "Network interface association with uevent file path"
                );
                uevent_file_path
            }
            Err(e) => {
                debug!(error = ?e, ?network_interface,
                    "uevent file not found for network interface"
                );
                continue;
            }
        };

        let is_ena = match is_ena_network_interface(&uevent_file_path) {
            Ok(is_ena) => {
                debug!(
                    ?network_interface,
                    ?is_ena,
                    "Network interface driver details"
                );
                is_ena
            }
            Err(e) => {
                debug!(error = ?e, ?network_interface,
                    "Failed to determine if network interface driver is ena",
                );
                continue;
            }
        };

        if !is_ena {
            // We only consider PTP devices attached to ENA network interfaces as in-scope
            // for use because this is the configuration used within Amazon Web Services.
            debug!(
                ?network_interface,
                ?is_ena,
                "Network interface does not use the ena driver.  Skipping.",
            );
            continue;
        }

        let pci_slot_name = match get_pci_slot_name(&uevent_file_path) {
            Ok(pci_slot_name) => {
                debug!(
                    ?network_interface,
                    ?pci_slot_name,
                    "Network interface association with PCI slot name",
                );
                pci_slot_name
            }
            Err(e) => {
                debug!(error = ?e, ?uevent_file_path,
                    "PCI slot name not found for uevent file path",
                );
                continue;
            }
        };

        let phc_clock_error_bound_sysfs_path =
            match get_phc_clock_error_bound_sysfs_path(&pci_slot_name) {
                Ok(phc_clock_error_bound_sysfs_path) => {
                    debug!(
                        ?network_interface,
                        ?phc_clock_error_bound_sysfs_path,
                        "Network interface association with PHC clock error bound sysfs path",
                    );
                    phc_clock_error_bound_sysfs_path
                }
                Err(e) => {
                    debug!(
                        error = ?e, ?pci_slot_name,
                        "PHC clock error bound sysfs path not found for PCI slot name",
                    );
                    continue;
                }
            };

        let ptp_uevent_file_paths = match get_ptp_uevent_file_paths_for_pci_slot(&pci_slot_name) {
            Ok(ptp_uevent_file_paths) => {
                debug!(
                    ?network_interface,
                    ?ptp_uevent_file_paths,
                    "Network interface association with PTP uevent file paths",
                );
                ptp_uevent_file_paths
            }
            Err(e) => {
                debug!(
                    error = ?e, ?pci_slot_name,
                    "PTP uevent file paths not found for PCI slot name",
                );
                continue;
            }
        };

        for ptp_uevent_file_path in ptp_uevent_file_paths {
            let ptp_device_name = match get_ptp_device_name_from_uevent_file(&ptp_uevent_file_path)
            {
                Ok(ptp_device_name) => {
                    debug!(
                        ?network_interface,
                        ?ptp_device_name,
                        "Network interface association with PTP device name",
                    );
                    ptp_device_name
                }
                Err(e) => {
                    debug!(
                        error = ?e, ?ptp_uevent_file_path,
                        "Device name not found for PTP uevent file path",
                    );
                    continue;
                }
            };

            let ptp_device_path = match get_ptp_device_path(&ptp_device_name) {
                Ok(ptp_device_path) => {
                    debug!(
                        ?network_interface,
                        ?ptp_device_path,
                        "Network interface association with PTP device path",
                    );
                    ptp_device_path
                }
                Err(e) => {
                    debug!(error = ?e, ?ptp_device_name,
                        "Device path not found for PTP device name",
                    );
                    continue;
                }
            };

            ptp_device_path_and_phc_clock_error_bound_sysfs_path_vec
                .push((ptp_device_path, phc_clock_error_bound_sysfs_path.clone()));
        }
    }

    // Sort the tuples in ascending order so that if there is more than one
    // PTP device, the lower numbered device names are preferred first. e.g.:
    //
    // [
    //    ("/dev/ptp0", "/sys/bus/pci/devices/0000:27:00.0/phc_error_bound"),
    //    ("/dev/ptp1", "/sys/bus/pci/devices/0000:28:00.0/phc_error_bound"),
    // ]
    ptp_device_path_and_phc_clock_error_bound_sysfs_path_vec.sort();
    debug!(?ptp_device_path_and_phc_clock_error_bound_sysfs_path_vec);

    // There is at least one PTP device available to use.
    // Use the first PTP device in the vec.
    if let Some((ptp_device_path, phc_clock_error_bound_sysfs_path)) =
        ptp_device_path_and_phc_clock_error_bound_sysfs_path_vec.first()
    {
        debug!(
            ?ptp_device_path,
            ?phc_clock_error_bound_sysfs_path,
            "Configuring PHC readers"
        );

        let ptp_device_file = match File::open(ptp_device_path.clone()) {
            Ok(file) => file,
            Err(e) => {
                let error_detail =
                    format!("Failed to open PTP device file: {ptp_device_path:?} {e:?}");
                return Err(PhcError::Io(io::Error::new(e.kind(), error_detail)));
            }
        };

        let phc_reader = PhcReader::new(
            ptp_device_file,
            PathBuf::from(phc_clock_error_bound_sysfs_path),
        );

        debug!(?phc_reader, "Done configuring PHC reader");
        Ok(phc_reader)
    } else {
        Err(PhcError::PtpDeviceNotFound(
            "No eligible PTP devices found".to_string(),
        ))
    }
}

/// Gets a list of network interface names on the host by inspecting
/// the files under the path "/sys/class/net/".
fn get_network_interfaces() -> Result<Vec<String>, PhcError> {
    let mut network_interfaces = Vec::new();
    let network_interfaces_path = "/sys/class/net/";

    // Validate the file path containing entries of the network interfaces exists.
    if !std::fs::exists(network_interfaces_path)? {
        return Err(PhcError::FileNotFound(network_interfaces_path.into()));
    }

    let entries = match std::fs::read_dir(network_interfaces_path) {
        Ok(entries) => entries,
        Err(e) => return Err(PhcError::Io(e)),
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return Err(PhcError::Io(e)),
        };

        let file_name = entry.file_name().to_string_lossy().to_string();
        network_interfaces.push(file_name);
    }

    tracing::debug!(?network_interfaces);
    Ok(network_interfaces)
}

/// Gets the uevent file path for a particular network interface.
fn get_uevent_file_path_for_network_interface(network_interface: &str) -> Result<String, PhcError> {
    let uevent_file_path = format!("/sys/class/net/{network_interface}/device/uevent");
    if !std::fs::exists(&uevent_file_path)? {
        return Err(PhcError::FileNotFound(uevent_file_path));
    }

    Ok(uevent_file_path)
}

/// Inspects the given uevent file for a network interface and determines if
/// the corresponding driver is "ena", which is the Amazon elastic network adapter.
fn is_ena_network_interface(uevent_file_path: &str) -> Result<bool, PhcError> {
    let contents = match std::fs::read_to_string(uevent_file_path) {
        Ok(contents) => contents,
        Err(e) => return Err(PhcError::Io(e)),
    };

    let driver_name = contents
        .lines()
        .find_map(|line| line.strip_prefix("DRIVER="))
        .ok_or_else(|| {
            PhcError::DeviceDriverNameNotFound(format!(
                "Failed to find DRIVER at uevent file path {uevent_file_path}"
            ))
        })
        .map(std::string::ToString::to_string)?;

    tracing::debug!(
        ?uevent_file_path,
        ?driver_name,
        "uevent file association with DRIVER value"
    );
    Ok(driver_name == "ena")
}

/// Gets the PCI slot name for a given network interface name.
///
/// # Arguments
///
/// * `uevent_file_path` - The path of the uevent file where we lookup the `PCI_SLOT_NAME`.
fn get_pci_slot_name(uevent_file_path: &str) -> Result<String, PhcError> {
    let contents = match std::fs::read_to_string(uevent_file_path) {
        Ok(contents) => contents,
        Err(e) => return Err(PhcError::Io(e)),
    };

    let pci_slot_name = contents
        .lines()
        .find_map(|line| line.strip_prefix("PCI_SLOT_NAME="))
        .ok_or_else(|| {
            PhcError::PciSlotNameNotFound(format!(
                "Failed to find PCI_SLOT_NAME at uevent file path {uevent_file_path}"
            ))
        })
        .map(std::string::ToString::to_string)?;

    tracing::debug!(
        ?uevent_file_path,
        ?pci_slot_name,
        "uevent file association with PCI_SLOT_NAME value"
    );
    Ok(pci_slot_name)
}

/// Gets the absolute file paths of the uevent files for PTP devices,
/// given the PCI slot name that corresponds to the ENA network interface.
///
/// File paths are expected to look like:
///   `/sys/bus/pci/devices/{pci_slot_name}/ptp/ptp0/uevent`,
///   `/sys/bus/pci/devices/{pci_slot_name}/ptp/ptp1/uevent`,
///   `/sys/bus/pci/devices/{pci_slot_name}/ptp/ptp2/uevent`,
///   etc.
fn get_ptp_uevent_file_paths_for_pci_slot(pci_slot_name: &str) -> Result<Vec<String>, PhcError> {
    let mut uevent_file_paths = Vec::new();
    let uevent_file_search_path = format!("/sys/bus/pci/devices/{pci_slot_name}/ptp/");

    if !std::fs::exists(&uevent_file_search_path)? {
        return Err(PhcError::FileNotFound(uevent_file_search_path));
    }

    let entries = match std::fs::read_dir(&uevent_file_search_path) {
        Ok(entries) => entries,
        Err(e) => return Err(PhcError::Io(e)),
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => return Err(PhcError::Io(e)),
        };

        let entry_name = entry.file_name().to_string_lossy().to_string();
        if entry_name.starts_with("ptp") {
            let uevent_path = format!("{uevent_file_search_path}{entry_name}/uevent");
            if std::fs::exists(&uevent_path)? {
                uevent_file_paths.push(uevent_path);
            }
        }
    }

    tracing::debug!(?uevent_file_paths, "PTP uevent file paths");
    Ok(uevent_file_paths)
}

/// Gets the PTP device name from the given `uevent_file_path`.
///
/// # Arguments
///
/// * `uevent_file_path` - The path of the uevent file where we lookup DEVNAME.
fn get_ptp_device_name_from_uevent_file(uevent_file_path: &str) -> Result<String, PhcError> {
    let contents = match std::fs::read_to_string(uevent_file_path) {
        Ok(contents) => contents,
        Err(e) => return Err(PhcError::Io(e)),
    };

    let ptp_device_name = contents
        .lines()
        .find_map(|line| line.strip_prefix("DEVNAME="))
        .ok_or_else(|| {
            PhcError::PtpDeviceNameNotFound(format!(
                "Failed to find DEVNAME at uevent file path {uevent_file_path}"
            ))
        })
        .map(std::string::ToString::to_string)?;

    tracing::debug!(
        ?uevent_file_path,
        ?ptp_device_name,
        "uevent file assocation with DEVNAME value"
    );
    Ok(ptp_device_name)
}

/// Gets the PTP device path for a particular PTP device name.
///
/// # Arguments
///
/// * `ptp_device_name` - The network interface to lookup the PHC error bound path for.
fn get_ptp_device_path(ptp_device_name: &str) -> Result<String, PhcError> {
    let ptp_device_path = format!("/dev/{ptp_device_name}");
    if !std::fs::exists(&ptp_device_path)? {
        return Err(PhcError::PtpDeviceNotFound(format!(
            "Failed to find PTP device at path {ptp_device_path}"
        )));
    }
    tracing::debug!(
        ?ptp_device_name,
        ?ptp_device_path,
        "PTP device name association with PTP device path"
    );
    Ok(ptp_device_path)
}

/// Gets the PHC Error Bound sysfs file path given a PCI slot name.
///
/// # Arguments
///
/// * `pci_slot_name` - The PCI slot name to use for constructing and locating the PHC clock error bound sysfs file.
fn get_phc_clock_error_bound_sysfs_path(pci_slot_name: &str) -> Result<String, PhcError> {
    let phc_clock_error_bound_sysfs_path =
        format!("/sys/bus/pci/devices/{pci_slot_name}/phc_error_bound");
    if !std::fs::exists(&phc_clock_error_bound_sysfs_path)? {
        return Err(PhcError::PhcClockErrorBoundFileNotFound {
            pci_slot_name: pci_slot_name.into(),
        });
    }
    tracing::debug!(
        ?pci_slot_name,
        ?phc_clock_error_bound_sysfs_path,
        "PCI slot name assocation with PHC clock error bound sysfs path"
    );
    Ok(phc_clock_error_bound_sysfs_path)
}
