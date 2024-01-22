// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
use crate::{response::build_response, socket, PhcInfo};
use chrony_candm::reply::Tracking;
use log::{error, warn};
use std::io;
use tokio::sync::watch::Receiver;
use uds::UnixDatagramExt;

/// The Unix Datagram Socket file for ClockBoundD
pub const CLOCKBOUND_SERVER_SOCKET: &str = "clockboundd.sock";

/// ClockBoundServer holds the Tracking data from Chrony and binds to the ClockBoundD unix socket
/// as a server.
pub struct ClockBoundServer {
    socket: std::os::unix::net::UnixDatagram,
    tracking: Tracking,
    phc_info: Option<PhcInfo>,
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
    pub fn new(tracking: Tracking, phc_info: Option<PhcInfo>) -> ClockBoundServer {
        let socket = socket::create_unix_socket(std::path::Path::new(CLOCKBOUND_SERVER_SOCKET));
        ClockBoundServer {
            socket,
            tracking,
            phc_info,
            phc_error_bound: 0_f64,
        }
    }

    /// Reads the PHC Error Bound and parses to a float.
    ///
    /// # Arguments
    ///
    /// * `phc_error_bound_path` - The path of sysfs file to read PHC error bound from.
    pub fn get_phc_error_bound(
        phc_error_bound_path: &std::path::Path,
    ) -> Result<f64, std::io::Error> {
        Ok(std::fs::read_to_string(phc_error_bound_path)?
            .trim()
            .parse::<f64>()
            .expect("Could not parse error bound value to f64"))
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
            match &self.phc_info {
                // If syncing to PHC, set PHC error bound too if possible.
                Some(phc_info) if phc_info.refid == tracking.ref_id => {
                    match ClockBoundServer::get_phc_error_bound(&phc_info.sysfs_error_bound_path) {
                        Ok(phc_error_bound) => {
                            self.phc_error_bound = phc_error_bound;
                            self.tracking = tracking;
                        }
                        Err(e) => error!("Failed to retrieve PHC error bound when Chrony was tracking PHC, error bound will not be updated: {:?}", e)
                    }
                }
                // If not syncing to PHC, PHC error bound should not be added in.
                _ => {
                    self.tracking = tracking;
                    self.phc_error_bound = 0_f64;
                }
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
