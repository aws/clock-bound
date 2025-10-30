//! NTP Server executable.
//!
//! This executable tests that the NTP Server runner is able to send and receive packets from the
//! specified NTP Server address and that the polling rate is roughly once a second.

use std::time::Duration;

use clock_bound::daemon::async_ring_buffer;
use clock_bound::daemon::io::{
    SourceIO,
    ntp::{FIRST_PUBLIC_TIME_ADDRESS, SECOND_PUBLIC_TIME_ADDRESS},
};

use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    // Create ring buffer channels for ntp io sources
    let (first_ntp_source_sender, first_ntp_source_receiver) = async_ring_buffer::create(1);
    let (second_ntp_source_sender, second_ntp_source_receiver) = async_ring_buffer::create(1);

    let mut sourceio = SourceIO::construct();

    // Create IO time sources
    sourceio
        .create_ntp_source(FIRST_PUBLIC_TIME_ADDRESS, first_ntp_source_sender)
        .await;
    sourceio
        .create_ntp_source(SECOND_PUBLIC_TIME_ADDRESS, second_ntp_source_sender)
        .await;
    println!("NTP Source creation complete!");

    // Get NTP packet from both specified servers
    println!("Lets get NTP packets!");
    sourceio.spawn_all();
    let timeout_err_msg = &format!(
        "Timeout was reached before a packet was received from {FIRST_PUBLIC_TIME_ADDRESS:#?}"
    );
    let event_result =
        tokio::time::timeout(Duration::from_secs(48), first_ntp_source_receiver.recv())
            .await
            .expect(timeout_err_msg);
    event_result.unwrap();
    println!("Packet received from first NTP host");

    let timeout_err_msg = &format!(
        "Timeout was reached before a packet was received from {SECOND_PUBLIC_TIME_ADDRESS:#?}"
    );
    let event_result =
        tokio::time::timeout(Duration::from_secs(48), second_ntp_source_receiver.recv())
            .await
            .expect(timeout_err_msg);
    event_result.unwrap();
    println!("Packet received from second NTP host\nTEST COMPLETE");
}
