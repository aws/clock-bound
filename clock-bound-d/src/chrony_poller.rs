// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
use chrony_candm::blocking_query;
use chrony_candm::reply::{ReplyBody, Tracking};
use chrony_candm::request::RequestBody;
use chrony_candm::ClientOptions;
use log::{error, warn};
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::str::FromStr;
use std::time::SystemTime;
use tokio::sync::watch::Sender;

/// The interval in seconds that we attempt to get an initial poll from chrony.
pub const CHRONY_TRACKING_INITIALIZE_INTERVAL: u64 = 10;

/// The interval in seconds that the chrony poller thread runs
pub const CHRONY_POLL_INTERVAL: u64 = 1;

/// A leap status value of 3 means unsynchronized.
pub const LEAP_STATUS_UNSYNCHRONIZED: u16 = 3;

/// Poll chronyd for tracking information.
pub fn poll() -> Option<Tracking> {
    let request_body = RequestBody::Tracking;

    // Chrony by default can be communicated with via localhost on port 323
    let server_addr = SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::from_str("::1").unwrap(),
        323,
        0,
        0,
    ));
    let result = blocking_query(request_body, ClientOptions::default(), &server_addr);

    match result {
        Err(e) => {
            error!("No reply from chronyd. Is it running? Error: {:?}", e);
            None
        }
        Ok(reply) => {
            if let ReplyBody::Tracking(body) = reply.body {
                Some(body)
            } else {
                error!(
                    "Reply from chronyd was invalid. Expected tracking data. Reply: {:?}",
                    reply
                );
                None
            }
        }
    }
}

/// Initialize tracking information from a poll to Chrony.
///
/// We need to initialize tracking information from a poll to chrony at least once before we can
/// start sending responses to clients. This ensures that chrony is running at startup and that
/// we can get initial tracking information to work with.
pub fn initialize_tracking() -> Tracking {
    loop {
        // Do an initial poll to initialize the tracking data before starting the Chrony poller
        // thread
        let result = poll();

        match result {
            Some(tracking) => {
                // When chronyd starts it takes a few seconds to initially synchronize. This check
                // makes sure that chronyd is initially synchronized before tracking data is
                // considered initialized. Otherwise there is a brief period when chronyd starts
                // that all tracking information will be default values.
                if tracking.leap_status != LEAP_STATUS_UNSYNCHRONIZED {
                    return tracking;
                }
                warn!(
                    "chronyd is reporting as unsynchronized. Starting clockboundd requires chronyd \
                 to be synchronized. Retrying..."
                );
            }
            None => error!("Unable to initialize tracking information from Chrony."),
        };

        // Sleep and retry. We don't want to constantly spam the logs with errors while we wait for
        // Chrony to startup.
        std::thread::sleep(std::time::Duration::from_secs(
            CHRONY_TRACKING_INITIALIZE_INTERVAL,
        ));
    }
}

/// Start the Chrony poller thread.
/// This thread obtains the tracking information from Chrony every second by default.
///
/// # Arguments
///
/// * `tx_tracking` - A tokio::sync::watch::channel sender handle that is used for sending Chrony
/// tracking information to the main thread.
/// * `tx_error_flag` - A tokio::sync::watch::channel sender handle that is used for sending an
/// error flag, indicating that the last Chrony poll failed, to the main thread.
pub fn start_chrony_poller(tx_tracking: Sender<Tracking>, tx_error_flag: Sender<bool>) {
    std::thread::spawn(move || loop {
        let result = poll();

        // If an error happens when polling Chrony, or attempting to send the tracking data to the
        // main thread, update the error_flag to true.
        let error_flag = match result {
            Some(tracking) => {
                // If chronyd is restarted it will report default values until it first syncs to a
                // source. If chronyd is reporting ref time as the Unix Epoch, then do not send
                // tracking information to clockboundd's main thread. This lets clockboundd continue
                // to use the last set of valid tracking data from chronyd instead of having the
                // clock error bound jump up until chronyd syncs to a source.
                if tracking.ref_time != SystemTime::UNIX_EPOCH {
                    // Send tracking data to the main thread
                    match tx_tracking.send(tracking.clone()) {
                        Ok(..) => false,
                        Err(e) => {
                            error!("Unable to send tracking data to main thread: {:?}", e);
                            true
                        }
                    }
                } else {
                    warn!(
                        "chronyd has not synced to a source since starting. Calculating error \
                    locally until chronyd synchronizes."
                    );
                    true
                }
            }
            None => true,
        };

        // Send error flag to the main thread
        match tx_error_flag.send(error_flag.clone()) {
            Ok(..) => (),
            Err(e) => error!("Unable to send Chrony error flag to main thread: {:?}", e),
        }

        std::thread::sleep(std::time::Duration::from_secs(CHRONY_POLL_INTERVAL));
    });
}
