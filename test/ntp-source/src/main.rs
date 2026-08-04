//! NTP Server executable.
//!
//! This executable tests that the NTP Server runner is able to send and receive packets from the
//! specified NTP Server address and that the polling rate is roughly once a second.

use clock_bound::daemon::AWS_TEMP_PUBLIC_TIME_ADDRESSES;
use clock_bound::daemon::async_ring_buffer::{self, BufferClosedError, Receiver};
use clock_bound::daemon::event::{Ntp, Stratum, ValidStratumLevel};
use clock_bound::daemon::io::ntp::NTPSourceSender;
use clock_bound::daemon::io::{SourceIO, ntp::DaemonInfo};
use clock_bound::daemon::selected_clock::{ClockSource, SelectedClockSource};
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use rand::{RngCore, rng};
use tracing_subscriber::EnvFilter;

/// Time out for waiting on source polling task to produce an NTP event
const TIMEOUT_SECS: u64 = 48; // Should yield 3 or more polls

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    test_ntp_sources().await;
    test_polls_with_selected_clock_source_combos().await;
}

/// Set up ntp source io for testing
async fn setup() -> (Vec<Receiver<Ntp>>, Arc<SelectedClockSource>, SourceIO) {
    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 100,
        startup_id: rng().next_u64(),
    };

    let selected_clock = Arc::new(SelectedClockSource::default());

    let mut sourceio = SourceIO::construct(selected_clock.clone(), daemon_info);
    let mut receiver_vec: Vec<Receiver<Ntp>> = Vec::new();

    for address in AWS_TEMP_PUBLIC_TIME_ADDRESSES {
        let (tx, rx) = async_ring_buffer::create(1);
        receiver_vec.push(rx);
        let sender_with_address: NTPSourceSender = (address, tx);
        sourceio.create_ntp_source(sender_with_address).await;
    }

    sourceio.spawn_all();
    (receiver_vec, selected_clock, sourceio)
}

/// Test basic NTP source functionality
async fn test_ntp_sources() {
    println!("Testing NTP sources ...");
    let (receiver_vec, _selected_clock, _sourceio) = setup().await;

    for i in 0..AWS_TEMP_PUBLIC_TIME_ADDRESSES.len() {
        #[allow(
            clippy::match_wild_err_arm,
            reason = "Only possible error from timeout is Elapsed, which has a private constructor"
        )]
        match tokio::time::timeout(Duration::from_secs(TIMEOUT_SECS), receiver_vec[i].recv()).await
        {
            Ok(Ok(_ntp_event)) => {
                println!(
                    "Packet received from host ({} / {})",
                    i + 1,
                    AWS_TEMP_PUBLIC_TIME_ADDRESSES.len()
                );
            }
            Ok(Err(BufferClosedError)) => {
                panic!(
                    "Buffer closed for NTP source {:#?}",
                    AWS_TEMP_PUBLIC_TIME_ADDRESSES[i]
                );
            }
            Err(_) => {
                panic!(
                    "Timeout reached before packet received from {:#?}",
                    AWS_TEMP_PUBLIC_TIME_ADDRESSES[i]
                );
            }
        }
    }
    println!("NTP sources test PASSED");
}

/// Test NTP sources with varied selected clock source combinations
async fn test_polls_with_selected_clock_source_combos() {
    println!("Testing polling with selected clock source combinations ...");

    let combinations = generate_selected_clock_combos();
    println!(
        "Generated {} selected clock source combinations",
        combinations.len()
    );

    let (receiver_vec, selected_clock, _sourceio) = setup().await;
    for (source, stratum) in combinations {
        // Set up the specific clock source and stratum
        match source.clone() {
            ClockSource::Init => {} // Default state
            ClockSource::Phc => selected_clock.set_to_phc(),
            ClockSource::VMClock => selected_clock.set_to_vmclock(),
            ClockSource::None => selected_clock.set_to_none(),
            ClockSource::Server(refid) => {
                selected_clock.set_to_server(IpAddr::V4(Ipv4Addr::from(refid)), stratum);
            }
        }

        // Clear any prior events from the buffer
        for receiver in &receiver_vec {
            while let Ok(Ok(_)) = timeout(Duration::from_millis(100), receiver.recv()).await {}
        }

        println!("Polling with selected clock: {selected_clock}");
        // First response from any NTP source is fine
        tokio::select! {
            result = receiver_vec[0].recv() => match result {
                Ok(_) => println!(" Polling succeeded."),
                Err(BufferClosedError) => panic!(" Polling failed: Buffer closed"),
            },
            result = receiver_vec[1].recv() => match result {
                Ok(_) => println!(" Polling succeeded. Selected clock: {selected_clock}"),
                Err(BufferClosedError) => panic!(" Polling failed: Buffer closed"),
            },
            () = tokio::time::sleep(Duration::from_secs(TIMEOUT_SECS)) => {
                panic!(" Polling failed: Timed out. Selected clock: {selected_clock}");
            }
        }
    }

    println!("Poll test with all selected clock source combinations PASSED");
}

fn generate_selected_clock_combos() -> Vec<(ClockSource, Stratum)> {
    let mut combos = Vec::new();

    let base_sources = [
        ClockSource::Init,
        ClockSource::Phc,
        ClockSource::VMClock,
        ClockSource::None,
        ClockSource::Server(0), // placeholder
    ];

    for source in base_sources {
        match source {
            ClockSource::Init => combos.push((ClockSource::Init, Stratum::Unspecified)),
            ClockSource::Phc => combos.push((ClockSource::Phc, Stratum::Unspecified)),
            ClockSource::VMClock => combos.push((ClockSource::VMClock, Stratum::Unspecified)),
            ClockSource::None => combos.push((ClockSource::None, Stratum::Unsynchronized)),
            ClockSource::Server(_) => {
                combos.push((
                    ClockSource::Server(u32::from_be_bytes([169, 254, 169, 123])),
                    Stratum::Level(ValidStratumLevel::new(1).unwrap()),
                ));
                combos.push((
                    ClockSource::Server({
                        let ipv6_bytes = "fd00:ec2::123"
                            .parse::<std::net::Ipv6Addr>()
                            .unwrap()
                            .octets();
                        let hash = md5::compute(ipv6_bytes);
                        u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]])
                    }),
                    Stratum::Level(ValidStratumLevel::new(1).unwrap()),
                ));
            }
        }
    }

    combos
}
