//! Module for collecting the phc samples and selecting the best sample

use super::tsc::{read_timestamp_counter_begin, read_timestamp_counter_end};
use super::{ClockDisruptionEvent, ControlRequest};
use crate::daemon::{
    async_ring_buffer::{self, SendError},
    event::{self, PhcData},
    time::tsc::TscCount,
    time::{Duration, Instant},
};
use libc::c_uint;
use nix::ioctl_readwrite;
use std::fs::File;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use thiserror::Error;
use tokio::{
    io,
    sync::{mpsc, watch},
    task,
    time::{Interval, MissedTickBehavior, interval, timeout},
};
use tracing::{debug, error, info, warn};

const PHC_SOURCE_INTERVAL_DURATION: std::time::Duration = tokio::time::Duration::from_millis(500);

/// Timeout for a single read of the PHC device.
const PHC_SOURCE_TIMEOUT: tokio::time::Duration = tokio::time::Duration::from_millis(20);

const NUM_SAMPLES_PER_POLL: u32 = 5;

/// `PTP_SYS_OFFSET_EXTENDED2` ioctl call.
const PTP_SYS_OFFSET_EXTENDED2: u32 = 3_300_932_882;

/// Maximum number of samples supported within a single `PTP_SYS_OFFSET_EXTENDED2` ioctl call.
const PTP_MAX_SAMPLES: usize = 25;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PtpClockTime {
    pub sec: i64,
    pub nsec: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct PtpSysOffsetExtended {
    /// Number of samples to collect
    pub n_samples: c_uint,
    /// Resevered
    pub rsv: [c_uint; 3],
    /// Array of samples in the form [pre-TS, PHC, post-TS ]
    pub ts: [[PtpClockTime; 3]; PTP_MAX_SAMPLES],
}

ioctl_readwrite!(
    ptp_sys_offset_extended2,
    b'=',
    PTP_SYS_OFFSET_EXTENDED2,
    PtpSysOffsetExtended
);

#[derive(Debug)]
pub enum TryFromPhcSampleError {
    InvalidCounterDiff,
    InvalidClockErrorBound,
    UnexpectedError,
}

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
    #[error("PHC clock error bound read failure")]
    PhcClockErrorBoundReadFailure(String),
    #[error("Tokio spawn JoinError")]
    AsyncTaskJoinError(task::JoinError),
    #[error("Device driver name not found")]
    DeviceDriverNameNotFound(String),
    #[error("PhcSample to event::Phc TryFrom error")]
    PhcSampleToPhcEventConversionError(TryFromPhcSampleError),
    #[error("Unexpected error")]
    UnexpectedError(String),
}

impl From<tokio::time::error::Elapsed> for PhcError {
    fn from(err: tokio::time::error::Elapsed) -> Self {
        PhcError::Io(io::Error::from(err))
    }
}

impl From<task::JoinError> for PhcError {
    fn from(err: task::JoinError) -> Self {
        PhcError::AsyncTaskJoinError(err)
    }
}

impl From<nix::Error> for PhcError {
    fn from(err: nix::Error) -> Self {
        PhcError::Io(io::Error::from_raw_os_error(err as i32))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PhcSample {
    pub counter_pre: u64,
    pub counter_post: u64,
    pub counter_diff: u64,

    /// Reference clock time
    pub ptp_clock_time: PtpClockTime,

    /// Clock error bound of this measurement
    pub ptp_clock_error_bound_nsec: i64,
}

impl TryFrom<PhcSample> for event::Phc {
    type Error = TryFromPhcSampleError;

    fn try_from(sample: PhcSample) -> Result<Self, Self::Error> {
        if sample.counter_post < sample.counter_pre {
            return Err(TryFromPhcSampleError::InvalidCounterDiff);
        }
        if sample.ptp_clock_error_bound_nsec < 0 {
            return Err(TryFromPhcSampleError::InvalidClockErrorBound);
        }

        let builder = event::Phc::builder()
            .tsc_pre(TscCount::new(sample.counter_pre.into()))
            .tsc_post(TscCount::new(sample.counter_post.into()))
            .data(PhcData {
                time: Instant::from_time(
                    sample.ptp_clock_time.sec.into(),
                    sample.ptp_clock_time.nsec,
                ),
                clock_error_bound: Duration::from_nanos(sample.ptp_clock_error_bound_nsec.into()),
            });

        match builder.build() {
            Some(phc) => Ok(phc),
            None => Err(TryFromPhcSampleError::UnexpectedError),
        }
    }
}

/// Contains data used to run PHC runner.
///
/// Notably the struct contains the path to the PHC device
#[derive(Debug)]
pub struct Phc {
    /// The message channel used to send PHC events.
    event_sender: async_ring_buffer::Sender<event::Phc>,
    /// The message channel used to receive control requests.
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    /// The message channel used to send clock disruption events.
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    /// The polling interval.
    interval: Interval,
    /// Reader for the PTP hardware clock (PHC).
    reader: PhcReader,
    /// Reader for the PTP hardware clock (PHC) clock error bound.
    clock_error_bound_reader: PhcClockErrorBoundReader,
    /// Path to the PHC device.
    device_path: String,
}

impl Phc {
    /// Constructs a new `Phc` IO source instance using the given parameters.
    ///
    /// This implementation performs autoconfiguration to determine
    /// the PHC device that should be used for obtaining reference clock
    /// timestamps and the corresponding clock error bound for those timestamps.
    pub async fn construct(
        event_sender: async_ring_buffer::Sender<event::Phc>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    ) -> Result<Self, PhcError> {
        let (phc_reader, phc_clock_error_bound_reader, device_path) =
            Self::autoconfigure_phc_readers().await?;
        let mut phc_interval = interval(PHC_SOURCE_INTERVAL_DURATION);
        phc_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        Ok(Phc {
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            interval: phc_interval,
            reader: phc_reader,
            clock_error_bound_reader: phc_clock_error_bound_reader,
            device_path,
        })
    }

    /// Getter for `device_path`
    pub fn device_path(&self) -> &str {
        &self.device_path
    }

    /// Attempts to autoconfigure readers for the PHC and PHC clock error bound
    /// by navigating and reading from the filesystem to obtain PTP device details.
    ///
    /// If there are no eligible PTP devices found then a `PhcError::PtpDeviceNotFound`
    /// will be returned in the Result.
    ///
    /// Returns the Phc Reader, the Phc Clock Error Bound Reader, and the selected ptp device path
    #[expect(
        clippy::too_many_lines,
        reason = "This function is already refactored to call
        separate functions for specific functionality.  The big for loop is needed
        because we have many `continue` statements in the loop body, and other alternate
        approaches would make the code harder to follow than the current implementation."
    )]
    pub async fn autoconfigure_phc_readers()
    -> Result<(PhcReader, PhcClockErrorBoundReader, String), PhcError> {
        // Get the list of network interfaces.
        let network_interfaces = match get_network_interfaces().await {
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

            let uevent_file_path =
                match get_uevent_file_path_for_network_interface(&network_interface).await {
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

            let is_ena = match is_ena_network_interface(&uevent_file_path).await {
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
                info!(
                    ?network_interface,
                    ?is_ena,
                    "Network interface does not use the ena driver.  Skipping.",
                );
                continue;
            }

            let pci_slot_name = match get_pci_slot_name(&uevent_file_path).await {
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
                match get_phc_clock_error_bound_sysfs_path(&pci_slot_name).await {
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

            let ptp_uevent_file_paths =
                match get_ptp_uevent_file_paths_for_pci_slot(&pci_slot_name).await {
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
                let ptp_device_name =
                    match get_ptp_device_name_from_uevent_file(&ptp_uevent_file_path).await {
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

                let ptp_device_path = match get_ptp_device_path(&ptp_device_name).await {
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
            ptp_device_path_and_phc_clock_error_bound_sysfs_path_vec
                .into_iter()
                .next()
        {
            info!(
                ?ptp_device_path,
                ?phc_clock_error_bound_sysfs_path,
                "Configuring PHC readers"
            );

            let ptp_device_file = match File::open(ptp_device_path.clone()) {
                Ok(file) => file,
                Err(e) => {
                    let error_detail = format!(
                        "Failed to open PTP device file: {:?} {:?}",
                        &ptp_device_path, e
                    );
                    return Err(PhcError::Io(io::Error::new(e.kind(), error_detail)));
                }
            };

            let phc_reader: PhcReader = PhcReader::new(ptp_device_file);
            let phc_clock_error_bound_reader: PhcClockErrorBoundReader =
                PhcClockErrorBoundReader::new(PathBuf::from(&phc_clock_error_bound_sysfs_path));

            info!(
                ?ptp_device_path,
                ?phc_clock_error_bound_sysfs_path,
                "Done configuring PHC readers"
            );
            Ok((phc_reader, phc_clock_error_bound_reader, ptp_device_path))
        } else {
            Err(PhcError::PtpDeviceNotFound(
                "No eligible PTP devices found".to_string(),
            ))
        }
    }

    /// Creates a `event::Phc` by reading from the PHC `NUM_SAMPLES_PER_POLL`
    /// times and retaining the best sample.
    async fn sample(&mut self) -> Result<event::Phc, PhcError> {
        let mut last_phc_error: Option<PhcError> = None;
        let mut best_sample: Option<PhcSample> = None;
        let mut best_sample_i: u32 = 0;

        debug!("Reading {:?} PHC samples ...", NUM_SAMPLES_PER_POLL);

        for i in 0..NUM_SAMPLES_PER_POLL {
            let sample = match self.read_phc_sample().await {
                Ok(sample) => {
                    debug!(?i, ?sample, "Successfully read PHC sample");
                    sample
                }
                Err(e) => {
                    warn!(?i, error = ?e, "PHC sample read failure");
                    last_phc_error = Some(e);
                    continue;
                }
            };

            // Retain the best sample.
            // The best sample is defined as the sample with the smallest
            // time stamp counter value difference.
            if let Some(bs) = best_sample {
                if sample.counter_diff < bs.counter_diff {
                    best_sample = Some(sample);
                    best_sample_i = i;
                }
            } else {
                best_sample = Some(sample);
                best_sample_i = i;
            }
        }

        if let Some(best_sample) = best_sample {
            debug!(?best_sample_i, ?best_sample);

            // Convert from PhcSample to event::Phc.
            match event::Phc::try_from(best_sample) {
                Ok(phc) => Ok(phc),
                Err(e) => Err(PhcError::PhcSampleToPhcEventConversionError(e)),
            }
        } else if let Some(last_phc_error) = last_phc_error {
            Err(last_phc_error)
        } else {
            // Should never get here.  We either should have a best sample
            // found, or there was some error encountered that was recorded.
            unreachable!("PHC IO source: unexpected unreachable");
        }
    }

    /// Reads a single PHC sample which consists of:
    /// - A reference clock timestamp
    /// - The corresponding clock error bound for that reference clock timestamp.
    /// - Timestamp stamp counter value prior to collecing the reference clock timestamp.
    /// - Timestamp stamp counter value after collecing the reference clock timestamp.
    ///
    /// If the `PhcReader` and `PhcClockErrorBoundReader` are not initialized, then this
    async fn read_phc_sample(&self) -> Result<PhcSample, PhcError> {
        let (ptp_clock_time, counter_pre, counter_post) = self
            .reader
            .ptp_sys_offset_extended2_with_counter_pre_and_counter_post(PHC_SOURCE_TIMEOUT)
            .await?;

        let ptp_clock_error_bound_nsec: i64 = match self.clock_error_bound_reader.read().await {
            Ok(ceb_nsec) => {
                if ceb_nsec < 0 {
                    return Err(PhcError::PhcClockErrorBoundReadFailure(format!(
                        "Negative PHC clock error bound: {ceb_nsec}"
                    )));
                }
                ceb_nsec
            }
            Err(e) => {
                return Err(PhcError::PhcClockErrorBoundReadFailure(e.clone()));
            }
        };

        Ok(PhcSample {
            counter_pre,
            counter_post,
            counter_diff: counter_post - counter_pre,
            ptp_clock_time,
            ptp_clock_error_bound_nsec,
        })
    }

    /// `PHC` task runner.
    ///
    /// Performs periodic sampling of the PHC IO source until a control event is received.
    ///
    /// # Panics
    /// Function will panic if not called within the `tokio` runtime.
    ///
    pub async fn run(&mut self) {
        // Sampling loop
        info!("Starting PHC sampling loop");
        loop {
            tokio::select! {
                biased; // priority order is disruption, commands, and ticks
                val = self.clock_disruption_receiver.changed() => {
                    if let Err(e) = val {
                        tracing::error!(?e, "Clock disruption receiver dropped.");
                        break;
                    }
                    // Clock Disruption logic here
                    info!("Received clock disruption signal.");
                    self.handle_disruption();
                }
                ctrl_req = self.ctrl_receiver.recv() => {
                    // Ctrl logic here.
                    match ctrl_req {
                        None => {
                            // this select can happen if `SourceIO` drops the ctrl_sender
                            break;
                        },
                        Some(ControlRequest::Shutdown) => {
                            info!("Received shutdown signal. Exiting.");
                            break;
                        },
                    }
                }
                _ = self.interval.tick() => {
                    self.handle_interval_tick().await;
                }
            }
        }
        info!("PHC runner exiting");
    }

    async fn handle_interval_tick(&mut self) {
        let phc_event = match self.sample().await {
            Ok(phc_event) => phc_event,
            Err(e) => {
                warn!(error = ?e, "Failed to sample PHC source");
                return;
            }
        };

        match self.event_sender.send(phc_event.clone()) {
            Ok(()) => debug!(?phc_event, "Successfully sent PHC IO event"),
            Err(SendError::Disrupted(_)) => {
                info!("Trying to send when there was a disruption event.");
            }
            Err(SendError::BufferClosed(e)) => {
                error!(?e, "PHC channel closed not supported in alpha");
                panic!("PHC IO source unable to communicate with daemon. {e:?}");
            }
        }
    }

    /// Handles a clock disruption event
    fn handle_disruption(&mut self) {
        let Self {
            clock_disruption_receiver: _,
            event_sender,
            ctrl_receiver: _,
            interval,
            reader: _,
            clock_error_bound_reader: _,
            device_path: _,
        } = self;
        // so interval immediately fires
        interval.reset();
        // and clean up disruption from sender side
        event_sender.handle_disruption();
        tracing::info!("PHC IO handled disruption");
    }
}

#[cfg_attr(test, mockall::automock)]
mod filesystem {
    /// Tests to see if the file exists.
    pub(crate) async fn fs_try_exists(path: &String) -> tokio::io::Result<bool> {
        tokio::fs::try_exists(path).await
    }
}

#[cfg(not(test))]
pub(crate) use filesystem::fs_try_exists;
#[cfg(test)]
pub(crate) use mock_filesystem::fs_try_exists;

#[cfg_attr(test, mockall::automock)]
mod phc_path_locator {
    use crate::daemon::io::phc::PhcError;
    use crate::daemon::io::phc::fs_try_exists;

    /// Gets a list of network interface names on the host by inspecting
    /// the files under the path "/sys/class/net/".
    pub(crate) async fn get_network_interfaces() -> Result<Vec<String>, PhcError> {
        let mut network_interfaces = Vec::new();
        let network_interfaces_path = "/sys/class/net/";

        // Validate the file path containing entries of the network interfaces exists.
        if !fs_try_exists(&network_interfaces_path.to_string()).await? {
            return Err(PhcError::FileNotFound(network_interfaces_path.into()));
        }

        let mut entries = match tokio::fs::read_dir(network_interfaces_path).await {
            Ok(entries) => entries,
            Err(e) => return Err(PhcError::Io(e)),
        };

        while let Some(entry) = match entries.next_entry().await {
            Ok(entry) => entry,
            Err(e) => return Err(PhcError::Io(e)),
        } {
            let file_name = entry.file_name().to_string_lossy().to_string();
            network_interfaces.push(file_name);
        }

        tracing::debug!(?network_interfaces);
        Ok(network_interfaces)
    }

    /// Gets the uevent file path for a particular network interface.
    pub(crate) async fn get_uevent_file_path_for_network_interface(
        network_interface: &str,
    ) -> Result<String, PhcError> {
        let uevent_file_path = format!("/sys/class/net/{network_interface}/device/uevent");
        if !fs_try_exists(&uevent_file_path).await? {
            return Err(PhcError::FileNotFound(uevent_file_path));
        }

        Ok(uevent_file_path)
    }

    /// Inspects the given uevent file for a network interface and determines if
    /// the corresponding driver is "ena", which is the Amazon elastic network adapter.
    pub(crate) async fn is_ena_network_interface(uevent_file_path: &str) -> Result<bool, PhcError> {
        let contents = match tokio::fs::read_to_string(uevent_file_path).await {
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
    pub(crate) async fn get_pci_slot_name(uevent_file_path: &str) -> Result<String, PhcError> {
        let contents = match tokio::fs::read_to_string(uevent_file_path).await {
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
    pub(crate) async fn get_ptp_uevent_file_paths_for_pci_slot(
        pci_slot_name: &str,
    ) -> Result<Vec<String>, PhcError> {
        let mut uevent_file_paths = Vec::new();
        let uevent_file_search_path = format!("/sys/bus/pci/devices/{pci_slot_name}/ptp/");

        if !fs_try_exists(&uevent_file_search_path).await? {
            return Err(PhcError::FileNotFound(uevent_file_search_path));
        }

        let mut entries = match tokio::fs::read_dir(&uevent_file_search_path).await {
            Ok(entries) => entries,
            Err(e) => return Err(PhcError::Io(e)),
        };

        while let Some(entry) = match entries.next_entry().await {
            Ok(entry) => entry,
            Err(e) => return Err(PhcError::Io(e)),
        } {
            let entry_name = entry.file_name().to_string_lossy().to_string();
            if entry_name.starts_with("ptp") {
                let uevent_path = format!("{uevent_file_search_path}{entry_name}/uevent");
                if fs_try_exists(&uevent_path).await? {
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
    pub(crate) async fn get_ptp_device_name_from_uevent_file(
        uevent_file_path: &str,
    ) -> Result<String, PhcError> {
        let contents = match tokio::fs::read_to_string(uevent_file_path).await {
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
    pub(crate) async fn get_ptp_device_path(ptp_device_name: &str) -> Result<String, PhcError> {
        let ptp_device_path = format!("/dev/{ptp_device_name}");
        if !fs_try_exists(&ptp_device_path).await? {
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
    pub(crate) async fn get_phc_clock_error_bound_sysfs_path(
        pci_slot_name: &str,
    ) -> Result<String, PhcError> {
        let phc_clock_error_bound_sysfs_path =
            format!("/sys/bus/pci/devices/{pci_slot_name}/phc_error_bound");
        if !fs_try_exists(&phc_clock_error_bound_sysfs_path).await? {
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
}

pub(crate) use phc_path_locator::{
    get_network_interfaces, get_pci_slot_name, get_phc_clock_error_bound_sysfs_path,
    get_ptp_device_name_from_uevent_file, get_ptp_device_path,
    get_ptp_uevent_file_paths_for_pci_slot, get_uevent_file_path_for_network_interface,
    is_ena_network_interface,
};

#[derive(Debug)]
pub struct PhcReader {
    /// PHC device file
    phc_device_file: File,
}

#[cfg_attr(test, mockall::automock)]
impl PhcReader {
    pub(crate) fn new(phc_device_file: File) -> Self {
        Self { phc_device_file }
    }

    pub(crate) async fn ptp_sys_offset_extended2_with_counter_pre_and_counter_post(
        &self,
        timeout_duration: tokio::time::Duration,
    ) -> Result<(PtpClockTime, u64, u64), PhcError> {
        let mut ptp_sys_offset_extended = PtpSysOffsetExtended {
            n_samples: 1,
            ..Default::default()
        };
        let phc_device_fd = self.phc_device_file.as_raw_fd();

        let result = timeout(
            timeout_duration,
            task::spawn_blocking(move || {
                let counter_pre = read_timestamp_counter_begin();
                // SAFETY: The ptp_sys_offset_extended2() function is generated by the
                // nix::ioctl_readwrite! macro and the call is safe because the arguments
                // are expected to be valid.  The file descriptor comes from a File
                // that had File::open() successfully called on the path, ensuring
                // that the file descriptor is valid.  The other argument provided to
                // the ptp_sys_offset_extended2() was created within this function
                // just above, and its definition matches the expected struct format.
                let result = unsafe {
                    ptp_sys_offset_extended2(phc_device_fd, &raw mut ptp_sys_offset_extended)
                };
                let ptp_clock_time = match result {
                    Ok(_) => ptp_sys_offset_extended.ts[0][1],
                    Err(e) => {
                        return Err(e);
                    }
                };
                let counter_post = read_timestamp_counter_end();
                Ok((ptp_clock_time, counter_pre, counter_post))
            }),
        )
        .await?;

        result?.map_err(PhcError::from)
    }
}

#[derive(Debug, Clone, Default)]
pub struct PhcClockErrorBoundReader {
    sysfs_phc_error_bound_path: PathBuf,
}

#[cfg_attr(test, mockall::automock)]
impl PhcClockErrorBoundReader {
    pub(crate) fn new(phc_clock_error_bound_path: PathBuf) -> Self {
        Self {
            sysfs_phc_error_bound_path: phc_clock_error_bound_path,
        }
    }

    pub(crate) async fn read(&self) -> Result<i64, String> {
        let contents = tokio::fs::read_to_string(&self.sysfs_phc_error_bound_path)
            .await
            .map_err(|e| e.to_string())?;
        contents
            .trim()
            .parse::<i64>()
            .map_err(|e| format!("Failed to parse PHC error bound value to i64: {e}"))
    }
}

#[cfg(test)]
mod test {
    use rstest::rstest;
    use tempfile::NamedTempFile;

    use super::*;
    use crate::daemon::event;

    use std::io::Write;

    #[tokio::test]
    #[rstest]
    #[case::happy_path("PCI_SLOT_NAME=12345", "12345")]
    #[case::happy_path_multi_line(
        "
oneline
PCI_SLOT_NAME=23456
twoline",
        "23456"
    )]
    async fn get_pci_slot_name_success(
        #[case] file_contents_to_write: &str,
        #[case] return_value: &str,
    ) {
        let mut test_uevent_file = NamedTempFile::new().expect("create mock uevent file failed");
        test_uevent_file
            .write_all(file_contents_to_write.as_bytes())
            .expect("write to mock uevent file failed");

        let rt =
            phc_path_locator::get_pci_slot_name(test_uevent_file.path().to_str().unwrap()).await;
        assert!(rt.is_ok());
        assert_eq!(rt.unwrap(), return_value.to_string());
    }

    #[tokio::test]
    #[rstest]
    #[case::missing_pci_slot_name("no pci slot name")]
    async fn get_pci_slot_name_failure(#[case] file_contents_to_write: &str) {
        let mut test_uevent_file = NamedTempFile::new().expect("create mock uevent file failed");
        test_uevent_file
            .write_all(file_contents_to_write.as_bytes())
            .expect("write to mock uevent file failed");

        let rt =
            phc_path_locator::get_pci_slot_name(test_uevent_file.path().to_str().unwrap()).await;
        assert!(rt.is_err());
        match rt.unwrap_err() {
            PhcError::PciSlotNameNotFound(_) => assert!(true),
            _ => assert!(false),
        };
    }

    #[tokio::test]
    async fn get_pci_slot_name_file_does_not_exist() {
        let rt = phc_path_locator::get_pci_slot_name("/does/not/exist").await;
        assert!(rt.is_err());
    }

    #[tokio::test]
    #[rstest]
    #[case::happy_path("12345", 12345)]
    async fn read_phc_error_bound_success(
        #[case] file_contents_to_write: &str,
        #[case] return_value: i64,
    ) {
        let mut test_phc_error_bound_file =
            NamedTempFile::new().expect("create mock phc error bound file failed");
        test_phc_error_bound_file
            .write_all(file_contents_to_write.as_bytes())
            .expect("write to mock phc error bound file failed");

        let phc_error_bound_reader =
            PhcClockErrorBoundReader::new(test_phc_error_bound_file.path().to_path_buf());
        let rt = phc_error_bound_reader.read().await;
        assert!(rt.is_ok());
        assert_eq!(rt.unwrap(), return_value);
    }

    #[tokio::test]
    #[rstest]
    #[case::parsing_fail("asdf_not_an_i64")]
    async fn read_phc_error_bound_bad_file_contents(#[case] file_contents_to_write: &str) {
        let mut test_phc_error_bound_file =
            NamedTempFile::new().expect("create mock phc error bound file failed");
        test_phc_error_bound_file
            .write_all(file_contents_to_write.as_bytes())
            .expect("write to mock phc error bound file failed");

        let phc_error_bound_reader =
            PhcClockErrorBoundReader::new(test_phc_error_bound_file.path().to_path_buf());
        let rt = phc_error_bound_reader.read().await;
        assert!(rt.is_err());
        assert!(
            rt.unwrap_err()
                .to_string()
                .contains("Failed to parse PHC error bound value to i64")
        );
    }

    #[tokio::test]
    async fn read_phc_error_bound_file_does_not_exist() {
        let phc_error_bound_reader = PhcClockErrorBoundReader::new("/does/not/exist".into());
        let rt = phc_error_bound_reader.read().await;
        assert!(rt.is_err());
    }

    #[test]
    fn phc_clock_error_bound_reader_new() {
        let path = PathBuf::from("/test/path");
        let reader = PhcClockErrorBoundReader::new(path.clone());
        assert_eq!(reader.sysfs_phc_error_bound_path, path);
    }

    #[test]
    fn phc_sample_try_from_success() {
        let sample = PhcSample {
            counter_pre: 100,
            counter_post: 200,
            counter_diff: 100,
            ptp_clock_time: PtpClockTime {
                sec: 1234567890,
                nsec: 123456789,
                reserved: 0,
            },
            ptp_clock_error_bound_nsec: 1000,
        };

        let result = event::Phc::try_from(sample);
        assert!(result.is_ok());
    }

    #[test]
    fn phc_sample_try_from_invalid_counter_diff() {
        let sample = PhcSample {
            counter_pre: 200,
            counter_post: 100,
            counter_diff: 0,
            ptp_clock_time: PtpClockTime {
                sec: 1234567890,
                nsec: 123456789,
                reserved: 0,
            },
            ptp_clock_error_bound_nsec: 1000,
        };

        let result = event::Phc::try_from(sample);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TryFromPhcSampleError::InvalidCounterDiff
        ));
    }

    #[test]
    fn phc_sample_try_from_invalid_clock_error_bound() {
        let sample = PhcSample {
            counter_pre: 100,
            counter_post: 200,
            counter_diff: 100,
            ptp_clock_time: PtpClockTime {
                sec: 1234567890,
                nsec: 123456789,
                reserved: 0,
            },
            ptp_clock_error_bound_nsec: -1000,
        };

        let result = event::Phc::try_from(sample);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            TryFromPhcSampleError::InvalidClockErrorBound
        ));
    }

    #[tokio::test]
    async fn get_network_interfaces_directory_not_found() {
        let ctx = mock_filesystem::fs_try_exists_context();
        ctx.expect().returning(|_| Ok(false));

        let result = phc_path_locator::get_network_interfaces().await;
        assert!(result.is_err());
        match result.unwrap_err() {
            PhcError::FileNotFound(path) => {
                assert_eq!(path, "/sys/class/net/");
            }
            _ => panic!("Expected FileNotFound error"),
        }
    }

    #[tokio::test]
    async fn get_uevent_file_path_for_network_interface_success() {
        let ctx = mock_filesystem::fs_try_exists_context();
        ctx.expect().returning(|_| Ok(true));

        let result = phc_path_locator::get_uevent_file_path_for_network_interface("eth0").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "/sys/class/net/eth0/device/uevent");
    }

    #[tokio::test]
    #[rstest]
    #[case::ena_driver("DRIVER=ena", true)]
    #[case::other_driver("DRIVER=e1000e", false)]
    #[case::multiline_ena("SUBSYSTEM=net\nDRIVER=ena\nOTHER=value", true)]
    async fn is_ena_network_interface_success(#[case] file_contents: &str, #[case] expected: bool) {
        let mut test_file = NamedTempFile::new().expect("create temp file failed");
        test_file
            .write_all(file_contents.as_bytes())
            .expect("write to temp file failed");

        let result =
            phc_path_locator::is_ena_network_interface(test_file.path().to_str().unwrap()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected);
    }

    #[tokio::test]
    async fn is_ena_network_interface_no_driver() {
        let mut test_file = NamedTempFile::new().expect("create temp file failed");
        test_file
            .write_all(b"SUBSYSTEM=net\nOTHER=value")
            .expect("write to temp file failed");

        let result =
            phc_path_locator::is_ena_network_interface(test_file.path().to_str().unwrap()).await;
        assert!(result.is_err());
        match result.unwrap_err() {
            PhcError::DeviceDriverNameNotFound(_) => {}
            _ => panic!("Expected DeviceDriverNameNotFound error"),
        }
    }

    #[tokio::test]
    #[ignore = "static mocks require synchronization"]
    async fn get_ptp_uevent_file_paths_for_pci_slot_directory_not_found() {
        let ctx = mock_filesystem::fs_try_exists_context();
        ctx.expect().returning(|_| Ok(false));

        let result = phc_path_locator::get_ptp_uevent_file_paths_for_pci_slot("test_slot").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            PhcError::FileNotFound(path) => {
                assert_eq!(path, "/sys/bus/pci/devices/test_slot/ptp/");
            }
            _ => panic!("Expected FileNotFound error"),
        }
    }

    #[tokio::test]
    #[rstest]
    #[case::simple_devname("DEVNAME=ptp0", "ptp0")]
    #[case::multiline_devname("SUBSYSTEM=ptp\nDEVNAME=ptp1\nOTHER=value", "ptp1")]
    async fn get_ptp_device_name_from_uevent_file_success(
        #[case] file_contents: &str,
        #[case] expected: &str,
    ) {
        let mut test_file = NamedTempFile::new().expect("create temp file failed");
        test_file
            .write_all(file_contents.as_bytes())
            .expect("write to temp file failed");

        let result = phc_path_locator::get_ptp_device_name_from_uevent_file(
            test_file.path().to_str().unwrap(),
        )
        .await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), expected);
    }

    #[tokio::test]
    async fn get_ptp_device_name_from_uevent_file_no_devname() {
        let mut test_file = NamedTempFile::new().expect("create temp file failed");
        test_file
            .write_all(b"SUBSYSTEM=ptp\nOTHER=value")
            .expect("write to temp file failed");

        let result = phc_path_locator::get_ptp_device_name_from_uevent_file(
            test_file.path().to_str().unwrap(),
        )
        .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            PhcError::PtpDeviceNameNotFound(_) => {}
            _ => panic!("Expected PtpDeviceNameNotFound error"),
        }
    }

    #[tokio::test]
    async fn get_phc_clock_error_bound_sysfs_path_not_found() {
        let ctx = mock_filesystem::fs_try_exists_context();
        ctx.expect().returning(|_| Ok(false));

        let result = phc_path_locator::get_phc_clock_error_bound_sysfs_path("0000:27:00.0").await;
        assert!(result.is_err());
        match result.unwrap_err() {
            PhcError::PhcClockErrorBoundFileNotFound { pci_slot_name } => {
                assert_eq!(pci_slot_name, "0000:27:00.0");
            }
            _ => panic!("Expected PhcClockErrorBoundFileNotFound error"),
        }
    }
}
