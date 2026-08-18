//! PHC test executable.
//!
//! This executable tests that the PHC runner is able to read timestamps and error bounds,
//! and that the rate of receiving PHC events closely matches the expected PHC polling rate.

use clock_bound::daemon::io::SourceIO;
use clock_bound::daemon::message::Dns as DnsMessage;
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

    println!("Lets get PHC samples!");
    let (phc_sender, phc_receiver) = async_ring_buffer::create(1);

    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 100,
        startup_id: rng().next_u64(),
    };

    let mut sourceio =
        SourceIO::construct(Arc::new(SelectedClockSource::default()), daemon_info, {
            let (tx, _) = tokio::sync::mpsc::channel::<DnsMessage>(1);
            tx
        });
    sourceio.create_phc(phc_sender).await;
    sourceio.spawn_all();

    let max_events_to_receive = 11;
    let mut total_polling_duration = time::Duration::from_secs(0);
    let mut num_events_received = 0;
    let mut start = time::Instant::now();

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
            "PHC event received.\n{phc_event:#?}\n{:?} msec",
            duration.as_millis()
        );

        // Skip the first sample, the IO runner will poll immediately after it's created.
        if i > 0 {
            total_polling_duration += duration;
            num_events_received += 1;
        }

        start = now;
    }

    let expected_poll_interval_duration = time::Duration::from_millis(500);
    println!("Expected poll interval duration: {expected_poll_interval_duration:?}");

    let actual_poll_interval_duration = total_polling_duration / num_events_received;
    println!("Actual poll interval duration (i.e. measured): {actual_poll_interval_duration:?}");

    let poll_interval_duration_margin_of_error_allowed = time::Duration::from_millis(30);
    println!(
        "Margin of error allowed for poll interval duration: {poll_interval_duration_margin_of_error_allowed:?}"
    );

    let poll_interval_difference_from_expected =
        actual_poll_interval_duration.abs_diff(expected_poll_interval_duration);
    println!(
        "Poll interval duration difference between actual and expected: {poll_interval_difference_from_expected:?}"
    );

    assert!(
        poll_interval_difference_from_expected < poll_interval_duration_margin_of_error_allowed
    );

    // Test is successful because all the assertions passed.
    println!("PHC test is successful.");
}
