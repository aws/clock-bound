// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
//! A daemon to provide clients with an error bounded timestamp interval.
//!
//! # Prerequisites
//!
//! [chronyd](https://chrony.tuxfamily.org/) must be running in order to run ClockBoundD. If running
//! on Amazon Linux 2, chronyd is already set as the default NTP daemon for you.
//!
//! If running on Amazon EC2, see the [EC2 User Guide](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/set-time.html) for more information on installing Chrony and syncing
//! to the Amazon Time Sync Service.
//!
//! # Installation
//! ## Cargo
//! ClockBoundD can be installed using Cargo. Instructions on how to install cargo can be found at
//! [doc.rust-lang.org](https://doc.rust-lang.org/cargo/getting-started/installation.html).
//!
//! If it's your first time installing Cargo on an AL2 EC2 instance you may need to also install gcc:
//! ```text
//! sudo yum install gcc
//! ```
//!
//! Run cargo install:
//! ```text
//! cargo install clock-bound-d
//! ```
//!
//! If cargo was installed with the rustup link above the default install location will be at
//! ```text
//! $HOME/.cargo/bin/clockboundd
//! ```
//!
//! ## Systemd configuration
//!
//! If built from source using cargo, it is recommended to set up systemd to manage ClockBoundD.
//!
//! Configuration Example:
//!
//! * Move binary to the location you want to run it from
//! ```text
//! sudo mv $HOME/.cargo/bin/clockboundd /usr/local/bin/clockboundd
//! ```
//!
//! * Create system user that systemd can use
//! ```text
//! sudo useradd -r clockbound
//! ```
//!
//! * Create unit file /usr/lib/systemd/system/clockboundd.service with the following contents
//! ```text
//! [Unit]
//! Description=ClockBoundD
//!
//! [Service]
//! Type=simple
//! Restart=always
//! RestartSec=10
//! ExecStart=/usr/local/bin/clockboundd
//! RuntimeDirectory=clockboundd
//! WorkingDirectory=/run/clockboundd
//! User=clockbound
//!
//! [Install]
//! WantedBy=multi-user.target
//! ```
//!
//! * Reload systemd
//! ```text
//! sudo systemctl daemon-reload
//! ```
//!
//! * Enable ClockBoundD to start at boot
//! ```text
//! sudo systemctl enable clockboundd
//! ```
//!
//! * Start ClockBoundD now
//! ```text
//! sudo systemctl start clockboundd
//! ```
//!
//! You can then check the status of the service with:
//! ```text
//! systemctl status clockboundd
//! ```
//!
//! # Usage
//!
//! To communicate with ClockBoundD a client is required. A rust client library exists at [ClockBoundC](../clock-bound-c/README.md)
//! that an application can use to communicate with ClockBoundD.
//!
//! ## Custom Client
//!
//! If you want to create a custom client see [Custom Client](../README.md#custom-client) for more information.
//!
//! # Logging
//!
//! By default, ClockBoundD logs to syslog at /var/log/daemon.log.
//!
//! syslog logs can be viewed with journalctl:
//! ```text
//! journalctl -u clockboundd
//! ```
//! # Updating README
//!
//! This README is generated via [cargo-readme](https://crates.io/crates/cargo-readme). Updating can be done by running:
//! ```text
//! cargo readme > README.md
//! ```
mod ceb;
mod chrony_poller;
mod response;
mod server;
mod socket;
mod tracking;

use crate::chrony_poller::start_chrony_poller;
use crate::server::ClockBoundServer;
use chrony_candm::reply::Tracking;
use log::{error, info};
use tokio::sync::watch;
use tokio::sync::watch::Receiver;

/// Constant for conversion from nanoseconds to seconds.
pub const NANOSEC_IN_SEC: u32 = 1_000_000_000;

/// Type alias for f64 for error bound values retrieved from PHC sysfs interface.
type PhcErrorBound = f64;

/// PhcInfo holds the refid of the PHC in chronyd (i.e. PHC0), and the path
/// of the sysfs error bound file to read from.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PhcInfo {
    pub refid: u32,
    pub sysfs_error_bound_path: std::path::PathBuf,
}

/// Helper for converting a string ref_id into a u32 for the chrony command protocol.
/// 
/// # Arguments
/// 
/// * `ref_id` - The ref_id as a string to be translated to a u32.
pub fn refid_to_u32(ref_id: &str) -> u32 {
    let bytes: Vec<u32> = ref_id.bytes().map(|val| val as u32).collect();
    bytes[0] << 24 | bytes[1] << 16 | bytes[2] << 8 | bytes[3]
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
    let uevent_path = format!("/sys/class/net/{}/device/uevent", interface);
    let contents = match std::fs::read_to_string(&uevent_path) {
        Ok(v) => v,
        Err(e) => {
            return Err(format!(
                "Failed to read contents of uevent file {} to string: {}",
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

/// Start ClockBoundD.
///
/// # Arguments
///
/// * `max_clock_error` - The assumed maximum frequency error that a system clock can gain between updates in ppm.
/// * `phc_info` - struct containing info on PHC interface and refid to use for error bound.
pub fn run(max_clock_error: f64, phc_info: Option<PhcInfo>) {
    info!("Initialized ClockBoundD");

    // Do an initial poll to initialize the tracking data before starting the Chrony poller
    // thread
    let tracking = chrony_poller::initialize_tracking();
    // Set up a channel for sending tracking data between threads
    let (tx_tracking, rx_tracking) = watch::channel(tracking);
    // An error flag used to inform the main thread if there was an error with the most recent
    // poll to Chrony. This flag will make it's way to clients via the response header.
    let error_flag = false;
    // Set up a channel for sending error flag data between threads
    let (tx_error_flag, rx_error_flag) = watch::channel(error_flag);
    // Initialize the server with initial tracking data
    let server = ClockBoundServer::new(tracking, phc_info);

    // Chrony poller thread
    start_chrony_poller(tx_tracking, tx_error_flag);
    info!("Initialized Chrony Poller thread");

    // Start main thread
    start_main_thread(server, rx_tracking, rx_error_flag, max_clock_error);
}

/// Start the main thread of ClockBoundD.
/// This thread processes any client requests that are received.
///
/// # Arguments
///
/// * `server` - A ClockBoundServer bound to our ClockBoundD Unix Socket.
/// * `rx_tracking` - A tokio::sync::watch::channel receiver handle that is used for receiving Chrony
/// tracking information from the Chrony Poller thread.
/// * `rx_error_flag` - A tokio::sync::watch::channel receiver handle that is used for receiving an
/// error flag, indicating that the last Chrony poll failed, from the Chrony Poller thread.
/// * `max_clock_error` - The assumed maximum frequency error that a system clock can gain between updates in ppm.
pub fn start_main_thread(
    mut server: ClockBoundServer,
    rx_tracking: Receiver<Tracking>,
    rx_error_flag: Receiver<bool>,
    max_clock_error: f64,
) {
    // Main thread
    loop {
        match server.handle_client(rx_tracking.clone(), rx_error_flag.clone(), max_clock_error) {
            Err(e) => error!("Failed to communicate with client. Error: {:?}", e),
            _ => {}
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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