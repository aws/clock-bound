//! NTP Server executable.
//!
//! This executable tests that the NTP Server runner is able to send and receive packets from the
//! specified NTP Server address and that the polling rate is roughly once a second.

use clock_bound::daemon::async_ring_buffer::{self, Receiver};
use clock_bound::daemon::event::Ntp;
use clock_bound::daemon::io::ntp::NTPSourceSender;
use clock_bound::daemon::io::{
    SourceIO,
    ntp::{AWS_TEMP_PUBLIC_TIME_ADDRESSES, DaemonInfo},
};
use std::time::Duration;

use rand::{RngCore, rng};
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 100,
        startup_id: rng().next_u64(),
    };

    let mut sourceio = SourceIO::construct(daemon_info);
    let mut receiver_vec: Vec<Receiver<Ntp>> = Vec::new();

    for address in AWS_TEMP_PUBLIC_TIME_ADDRESSES {
        // Create ring buffer channels for ntp io sources
        let (tx, rx) = async_ring_buffer::create(1);
        receiver_vec.push(rx);

        // Create IO time sources
        let sender_with_address: NTPSourceSender = (address, tx);
        sourceio.create_ntp_source(sender_with_address).await;
    }
    println!("NTP Source creation complete!");

    // Get NTP packet from both specified servers
    println!("Lets get NTP packets!");
    sourceio.spawn_all();
    for i in 0..AWS_TEMP_PUBLIC_TIME_ADDRESSES.len() {
        let timeout_err_msg = &format!(
            "Timeout was reached before a packet was received from {:#?}",
            AWS_TEMP_PUBLIC_TIME_ADDRESSES[i]
        );

        let event_result = tokio::time::timeout(Duration::from_secs(48), receiver_vec[i].recv())
            .await
            .expect(timeout_err_msg);

        event_result.unwrap();
        println!(
            "Packet received from host ({} / {})",
            i + 1,
            AWS_TEMP_PUBLIC_TIME_ADDRESSES.len()
        );
    }
    println!("TEST COMPLETE");
}
