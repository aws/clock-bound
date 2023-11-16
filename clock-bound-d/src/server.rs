// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
use crate::{response::build_response, socket, PhcInfo};
use chrony_candm::reply::Tracking;
use log::warn;
use std::io::{self, Read, Error, ErrorKind};
use tokio::sync::watch::Receiver;
use uds::UnixDatagramExt;
use log::error;

/// The Unix Datagram Socket file for ClockBoundD
pub const CLOCKBOUND_SERVER_SOCKET: &str = "clockboundd.sock";

/// ClockBoundServer holds the Tracking data from Chrony and binds to the ClockBoundD unix socket
/// as a server.
pub struct ClockBoundServer {
    socket: std::os::unix::net::UnixDatagram,
    tracking: Tracking,
    phc_error_bound_path: Option<std::path::PathBuf>,
    phc_refid: Option<u32>,
    phc_error_bound: f64,
}

impl ClockBoundServer {
    /// Initialize ClockBound to contain default Tracking information and bind to the ClockBoundD
    /// unix socket.
    ///
    /// # Arguments
    ///
    /// * `tracking` - The tracking information received from Chrony.
    /// * `phc_info` - Optional - If PHC command line args are supplied (interface and ref ID), then this PhcInfo
    /// is used for determining the path at which to grab the PHC error bound data.
    pub fn new(tracking: Tracking, phc_info: Option<PhcInfo>) -> Result<ClockBoundServer, io::Error> {
        let socket = socket::create_unix_socket(std::path::Path::new(CLOCKBOUND_SERVER_SOCKET));

        if let Some(p) = phc_info {
            let phc_refid = p.refid;
            let phc_error_bound_path = match ClockBoundServer::get_error_bound_sysfs_path(p.interface) {
                Ok(v) => v,
                Err(e) => {
                    return Err(
                        Error::new(
                            ErrorKind::InvalidInput,
                            format!("Failed to get PHC error bound sysfs path: {}", e),
                        )
                    );
                }
            };
            let phc_error_bound = ClockBoundServer::get_phc_error_bound(&phc_error_bound_path);
            Ok(ClockBoundServer { socket, tracking, phc_refid: Some(phc_refid), phc_error_bound_path: Some(phc_error_bound_path), phc_error_bound })
        } else {
            Ok(ClockBoundServer { socket, tracking,phc_refid: None, phc_error_bound_path: None, phc_error_bound: 0_f64 })
        }
    }

    /// Gets the PHC Error Bound sysfs file path given a network interface name.
    /// 
    /// # Arguments
    /// 
    /// * `interface` - The network interface to lookup the PHC error bound path for.
    pub fn get_error_bound_sysfs_path(interface: String) -> Result<std::path::PathBuf, io::Error> {
        let uevent_path = format!("/sys/class/net/{}/device/uevent", interface);
        let mut contents = String::new();
        std::fs::File::open(uevent_path)?.read_to_string(&mut contents)?;
        let pci_slot_name = contents
            .lines()
            .find_map(|line| {
                line.strip_prefix("PCI_SLOT_NAME=")
            })
            .ok_or(Error::new(
                ErrorKind::InvalidInput,
                format!("Failed to find PCI_SLOT_NAME for interface {}", interface),
            ))?;
        Ok(std::path::PathBuf::from(format!("/sys/bus/pci/devices/{}/phc_error_bound", pci_slot_name)))
    }

    /// Reads the PHC Error Bound and parses to a float.
    ///
    /// # Arguments
    ///
    /// * `phc_error_bound_path` - The path of sysfs file to read PHC error bound from.
    pub fn get_phc_error_bound(phc_error_bound_path: &std::path::Path) -> f64 {
        let mut contents = String::new();
        std::fs::File::open(phc_error_bound_path)
            .expect("Could not open PHC error bound path")
            .read_to_string(&mut contents)
            .expect("Could not read PHC error bound file contents to str");
        contents.trim().parse::<f64>().expect("Could not parse error bound value to f64")
    }

    /// Update Tracking data, and if we have received a new Tracking object, read PHC error bound and
    /// update it in the server state. If the current tracking info of the ClockBoundServer indicates 
    /// that we are not currently syncing to the PHC, or if the PHC refid and interface were not supplied,
    /// then the phc_error_bound is set to 0.
    /// 
    /// # Arguments
    ///
    /// * `tracking` - The tracking information received from Chrony.
    pub fn update_tracking(&mut self, tracking: Tracking) {
        if self.tracking != tracking {
            self.tracking = tracking;
            self.phc_error_bound = match (&self.phc_error_bound_path, &self.phc_refid) {
                (Some(error_bound_path), Some(phc_refid)) if self.tracking.ref_id == *phc_refid => ClockBoundServer::get_phc_error_bound(&error_bound_path),
                _ => 0_f64
            }
        }
    }

    /// Handle a request from a client.
    ///
    /// # Arguments
    ///
    /// * `rx_tracking` - A tokio::sync::watch::channel receiver handle that is used for receiving Chrony
    /// tracking information from the Chrony Poller thread.
    /// * `rx_error_flag` - A tokio::sync::watch::channel receiver handle that is used for receiving an
    /// error flag, indicating that the last Chrony poll failed, from the Chrony Poller thread.
    /// * `max_clock_error` - The assumed maximum frequency error that a system clock can gain between updates in ppm.
    pub fn handle_client(
        &mut self,
        rx_tracking: Receiver<Tracking>,
        rx_error_flag: Receiver<bool>,
        max_clock_error: f64,
    ) -> Result<(), io::Error> {
        let mut request: [u8; 12] = [0; 12];

        let (request_size, client) = self.socket.recv_from_unix_addr(&mut request)?;

        // Get tracking data from chrony poller thread
        let tracking = *rx_tracking.borrow();

        // Update the tracking information with the latest information from the chrony poller thread
        self.update_tracking(tracking.clone());

        // Get error flag from chrony poller thread
        let error_flag = *rx_error_flag.borrow();

        let mut response: Vec<u8> = build_response(
            request,
            request_size,
            self.tracking,
            self.phc_error_bound,
            error_flag,
            max_clock_error,
        );

        if let Err(e) = self.socket.send_to_unix_addr(&mut response, &client) {
            warn!("Failed to send response to client. Error: {:?}", e);
        }

        Ok(())
    }
}
