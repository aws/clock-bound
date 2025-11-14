//! PHC test executable.
//!
//! This executable tests that the PHC runner is able to read timestamps and error bounds,
//! and that the polling rate is roughly once a second.

use clock_bound::daemon::io::SourceIO;
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

    println!("Lets get a PHC sample!");
    let (phc_sender, phc_receiver) = async_ring_buffer::create(1);

    let mut start = time::Instant::now();

    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 100,
        startup_id: rng().next_u64(),
    };

    let mut sourceio = SourceIO::construct(Arc::new(SelectedClockSource::default()), daemon_info);
    sourceio.create_phc(phc_sender).await;
    sourceio.spawn_all();

    let max_events_to_receive = 11;

    let mut total_polling_duration = time::Duration::from_secs(0);
    let mut num_events_received = 0;

    for i in 0..max_events_to_receive {
        // PHC events are expected to be received periodically.
        // Use a timeout() to prevent the runner from running forever if there happens
        // to be an issue with the PHC or there is a bug that prevents events from
        // being sent to us.
        let phc_event = timeout(Duration::from_secs(5), phc_receiver.recv())
            .await
            .unwrap();
        let now = time::Instant::now();
        let duration = now - start;
        println!(
            "It looks like we got an PHC sample \n{phc_event:#?}\n{:?} msec",
            duration.as_millis()
        );

        // Skip the first sample, the IO runner will poll immediately after it's created.
        if i > 0 {
            total_polling_duration += duration;
            num_events_received += 1;
        }

        start = now;
    }
    let polling_rate = total_polling_duration / num_events_received;
    println!("Polling rate avg: {polling_rate:?}");
    assert!(polling_rate.abs_diff(time::Duration::from_secs(1)) < time::Duration::from_millis(100));
}
