// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
use crate::response::build_response;
use crate::socket;
use chrony_candm::reply::Tracking;
use log::warn;
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
}

impl ClockBoundServer {
    /// Initialize ClockBound to contain default Tracking information and bind to the ClockBoundD
    /// unix socket.
    ///
    /// # Arguments
    ///
    /// * `tracking` - The tracking information received from Chrony.
    pub fn new(tracking: Tracking) -> ClockBoundServer {
        let socket = socket::create_unix_socket(std::path::Path::new(CLOCKBOUND_SERVER_SOCKET));

        return ClockBoundServer { socket, tracking };
    }

    /// Update Tracking data.
    ///
    /// # Arguments
    ///
    /// * `tracking` - The tracking information received from Chrony.
    pub fn update_tracking(&mut self, tracking: Tracking) {
        self.tracking = tracking;
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
            error_flag,
            max_clock_error,
        );

        if let Err(e) = self.socket.send_to_unix_addr(&mut response, &client) {
            warn!("Failed to send response to client. Error: {:?}", e);
        }

        Ok(())
    }
}
