//! NTP Server executable.
//!
//! This executable tests that the NTP Server runner is able to send and receive packets from the
//! specified NTP Server address and that the polling rate is roughly once a second.

use clock_bound::daemon::async_ring_buffer;
use clock_bound::daemon::io::SourceIO;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time;

use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    println!("Lets get a NTP packet!");
    let (first_ntp_source_sender, first_ntp_source_receiver) = async_ring_buffer::create(1);
    let (second_ntp_source_sender, second_ntp_source_receiver) = async_ring_buffer::create(1);

    let mut start = time::Instant::now();

    let mut sourceio = SourceIO::construct();
    // Currently hardcoded with a public NTP server IP
    // TODO: Update this IP to new AGA IP after resource provisioning is complete
    let first_ntp_source_public_ip =
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(54, 210, 225, 137)), 123);
    let second_ntp_source_public_ip =
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(3, 33, 208, 232)), 123);

    sourceio
        .create_ntp_source(first_ntp_source_public_ip, first_ntp_source_sender)
        .await;
    sourceio
        .create_ntp_source(second_ntp_source_public_ip, second_ntp_source_sender)
        .await;
    println!("NTP Server creation complete!");

    let mut polling_rate = time::Duration::from_secs(0);
    for i in 0..6 {
        println!("Polling {i}");

        // Get NTP packet from both specified servers
        let ntpevent_a = first_ntp_source_receiver.recv().await;
        let ntpevent_b = second_ntp_source_receiver.recv().await;

        let now = time::Instant::now();
        let d = now - start;
        println!(
            "It looks like we got an ntp packet from 1st Source \n{ntpevent_a:#?}\n{:?} ms",
            d.as_millis()
        );
        println!(
            "It looks like we got an ntp packet from 2nd Source \n{ntpevent_b:#?}\n{:?} ms",
            d.as_millis()
        );

        // Skip the first sample, the IO runner will poll immediately after it's created.
        if i > 0 {
            polling_rate += d;
        }

        start = now;
    }
    polling_rate /= 5;
    println!("Polling rate avg: {polling_rate:?}");
    assert!(
        polling_rate.abs_diff(time::Duration::from_secs(16)) < time::Duration::from_millis(200)
    );
}
