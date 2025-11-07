//! Link Local test executable.
//!
//! This executable tests that the link local runner is able to send and receive packets from the
//! link local address and that the polling rate is roughly once a second.

use clock_bound::daemon::event::Ntp;
use clock_bound::daemon::io::{SourceIO, ntp::LINK_LOCAL_BURST_INTERVAL_DURATION};
use clock_bound::daemon::selected_clock::SelectedClockSource;
use clock_bound::daemon::{async_ring_buffer, io::ntp::DaemonInfo};
use std::sync::Arc;
use std::time;

use rand::{RngCore, rng};
use tokio::time::{Duration, timeout};
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    println!("Lets get a NTP packet!");
    let (link_local_sender, mut link_local_receiver) = async_ring_buffer::create(1);

    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 100,
        startup_id: rng().next_u64(),
    };

    let mut sourceio = SourceIO::construct(Arc::new(SelectedClockSource::default()), daemon_info);
    sourceio.create_link_local(link_local_sender).await;
    sourceio.spawn_all();

    validate_burst_mode(&mut link_local_receiver).await;
    validate_normal_mode(&mut link_local_receiver).await;
}

async fn validate_burst_mode(receiver: &mut async_ring_buffer::Receiver<Ntp>) {
    let start = time::Instant::now();
    let mut polling_rate = time::Duration::from_secs(0);
    let mut count = 0;
    loop {
        // On instances that aren't able to connect to link local the runner will run infinitely.
        // To address this we timeout if an NTP event has not been received.
        let lap_start = time::Instant::now();
        let ntpevent = timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap();
        let now = time::Instant::now();
        let d = now - lap_start;
        println!(
            "It looks like we got an ntp packet \n{ntpevent:#?}\n{:?} ms",
            d.as_millis()
        );

        // Skip the first sample, the IO runner will poll immediately after it's created.
        if count == 0 {
            count += 1;
        } else {
            polling_rate += d;
            count += 1;
        }

        if start.elapsed() >= Duration::from_secs(1) {
            break;
        }
    }
    polling_rate /= count - 1;
    println!("Burst Polling rate avg: {polling_rate:?}");
    assert!(
        polling_rate.abs_diff(LINK_LOCAL_BURST_INTERVAL_DURATION)
            < time::Duration::from_millis(100)
    );
}

async fn validate_normal_mode(receiver: &mut async_ring_buffer::Receiver<Ntp>) {
    let mut start = time::Instant::now();
    let mut polling_rate = time::Duration::from_secs(0);
    for i in 0..11 {
        // On instances that aren't able to connect to link local the runner will run infinitely.
        // To address this we timeout if an NTP event has not been received.
        let ntpevent = timeout(Duration::from_secs(5), receiver.recv())
            .await
            .unwrap();
        let now = time::Instant::now();
        let d = now - start;
        println!(
            "It looks like we got an ntp packet \n{ntpevent:#?}\n{:?} ms",
            d.as_millis()
        );

        // Skip the first sample, the IO runner will poll immediately after it's created.
        if i > 0 {
            polling_rate += d;
        }

        start = now;
    }
    polling_rate /= 10;
    println!("Polling rate avg: {polling_rate:?}");
    assert!(polling_rate.abs_diff(time::Duration::from_secs(2)) < time::Duration::from_millis(100));
}
