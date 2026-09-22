//! Replay a repro logfile against the current `ClockSyncAlgorithm` and see how it goes
//!
//! Prints the new outputs into another log-directory
//!
//! # Sources
//!
//! The algorithm starts with no source and grows one per source seen in the log: the first
//! event from the PHC, from the Amazon Time Sync address, and from each NTP server address
//! creates the matching source before being fed. (Sources are listed, here and in the code, in
//! the order of their expected accuracy: PHC, then EC2 link-local NTP, then other NTP servers).
//! This reproduces the daemon's own source set for a capture the daemon made (it only polls what it
//! is configured with, so a configured server that never answered is, for replay purposes, not
//! configured), and uses every polled source for captures made by other collectors. Source creation
//! order is irrelevant: the sources are independent and the selector compares their current bounds.

use std::{path::PathBuf, sync::Arc};

use crate::{
    logging,
    repro::{self, ReproEvent},
};
use clap::Parser;
use clock_bound::daemon::{
    clock_sync_algorithm::{ClockSyncAlgorithm, FfSyncParams, Selector, source},
    receiver_stream::RoutableEvent,
    selected_clock::SelectedClockSource,
};

#[derive(Parser, Clone, Debug)]
/// Replay a scenario from a logfile back into the `ClockSyncAlgorithm`
pub struct Cli {
    /// The logfile to replay
    #[arg(short, long)]
    logfile: PathBuf,

    /// The output log directory
    #[arg(short, long)]
    output_dir: PathBuf,
}

/// Make sure the source an event is addressed to exists, creating it the first time it is
/// encountered in the log replay.
///
/// Returns `true` if a source was created.
fn ensure_source(
    alg: &mut ClockSyncAlgorithm,
    event: &RoutableEvent,
    ff_sync_params: &FfSyncParams,
) -> bool {
    match event {
        RoutableEvent::Phc(device_path, _) => {
            // The algorithm holds one PHC; every PHC event is routed to it whatever its path.
            if alg.phc().is_some() {
                return false;
            }
            alg.set_phc(source::Phc::new(device_path.clone(), ff_sync_params));
            tracing::info!(%device_path, "Created PHC source");
        }
        RoutableEvent::AmazonTimeSync(_) => {
            if alg.amazon_time_sync().is_some() {
                return false;
            }
            alg.set_amazon_time_sync(source::AmazonTimeSync::new(ff_sync_params));
            tracing::info!("Created Amazon Time Sync source");
        }
        RoutableEvent::NtpSource(addr, _) => {
            if alg
                .ntp_sources()
                .iter()
                .any(|s| s.socket_address() == *addr)
            {
                return false;
            }
            alg.add_ntp_source(source::NtpSource::new_ip_addr_source(*addr, ff_sync_params));
            tracing::info!(%addr, "Created NTP source");
        }
    }
    true
}

pub fn main(args: Cli) -> anyhow::Result<()> {
    // TODO: for now, pass the same default values as the daemon, instrumentation is next.
    let ff_sync_params = FfSyncParams::default();
    // No source yet: they are created from the log as they appear
    let mut alg = ClockSyncAlgorithm::builder()
        .selected_clock(Arc::new(SelectedClockSource::default()))
        .selector(Selector::new(ff_sync_params.max_dispersion))
        .build();

    // Needed to output updated clock estimates and internal algorithm calculations to their
    // respective log files.
    logging::init_for_synchronization_replay(args.output_dir);

    let mut repro_events = repro::repro_events_from_log_file(&args.logfile)?;

    // Stream events straight from the log: memory usage is bounded by the longest
    // line, not the file size. Parse errors abort the replay, as a partially
    // replayed log would produce misleading analysis data.
    let (mut feeds, mut inits, mut disruptions, mut sources) = (0usize, 0usize, 0usize, 0usize);
    for event in &mut repro_events {
        match event? {
            ReproEvent::Init => {
                inits += 1;
                tracing::warn!("Received init."); // TODO: Better breakdown into scenarios
            }
            ReproEvent::Disruption => {
                disruptions += 1;
                tracing::warn!("Received disruption.");
                alg.handle_disruption();
            }
            ReproEvent::Feed { event, .. } => {
                feeds += 1;
                if ensure_source(&mut alg, &event, &ff_sync_params) {
                    sources += 1;
                }
                // unused result since output log directory will contain this
                let _ = alg.feed(event);
            }
        }
    }
    anyhow::ensure!(
        feeds + inits + disruptions > 0,
        "No events found in log file"
    );

    tracing::info!(
        lines_read = repro_events.lines_read(),
        events = feeds + inits + disruptions,
        feeds,
        inits,
        disruptions,
        sources,
        ordering = %repro_events.order_summary(),
        "replay complete"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repro::repro_events_from_reader;

    fn empty_alg() -> ClockSyncAlgorithm {
        let ff_sync_params = FfSyncParams::default();
        ClockSyncAlgorithm::builder()
            .selected_clock(Arc::new(SelectedClockSource::default()))
            .selector(Selector::new(ff_sync_params.max_dispersion))
            .build()
    }

    /// Feed events of the parser's fixture: each source is created exactly once, on first sight.
    #[test]
    fn sources_are_created_once_from_the_log() {
        let ff_sync_params = FfSyncParams::default();
        let events =
            repro_events_from_reader(include_str!("../repro/test_ffevents.log").as_bytes())
                .unwrap();
        let feeds: Vec<RoutableEvent> = events
            .into_iter()
            .filter_map(|e| match e {
                ReproEvent::Feed { event, .. } => Some(event),
                _ => None,
            })
            .collect();
        assert!(feeds.len() >= 2, "fixture must carry feed events");

        let mut alg = empty_alg();
        assert!(
            alg.amazon_time_sync().is_none() && alg.phc().is_none() && alg.ntp_sources().is_empty()
        );

        let created: usize = feeds
            .iter()
            .map(|e| usize::from(ensure_source(&mut alg, e, &ff_sync_params)))
            .sum();
        let distinct = {
            let mut ids: Vec<String> = feeds
                .iter()
                .map(|e| match e {
                    RoutableEvent::Phc(_, _) => "phc".to_owned(),
                    RoutableEvent::AmazonTimeSync(_) => "ats".to_owned(),
                    RoutableEvent::NtpSource(addr, _) => addr.to_string(),
                })
                .collect();
            ids.sort();
            ids.dedup();
            ids.len()
        };
        assert_eq!(created, distinct, "one source per distinct identifier");

        // A second pass creates nothing.
        let again: usize = feeds
            .iter()
            .map(|e| usize::from(ensure_source(&mut alg, e, &ff_sync_params)))
            .sum();
        assert_eq!(again, 0);

        // And every event can now be fed without hitting the "no PHC built" invariant.
        for e in feeds {
            let _ = alg.feed(e);
        }
    }

    #[test]
    fn ntp_sources_are_keyed_by_address() {
        let ff_sync_params = FfSyncParams::default();
        let mut alg = empty_alg();
        let ntp = |port: u16| {
            RoutableEvent::NtpSource(
                std::net::SocketAddr::from(([10, 0, 0, 1], port)),
                clock_bound::daemon::event::Ntp::builder()
                    .counter_pre(clock_bound::daemon::time::TscCount::new(1_000))
                    .counter_post(clock_bound::daemon::time::TscCount::new(2_000))
                    .ntp_data(clock_bound::daemon::event::NtpData {
                        server_recv_time: clock_bound::daemon::time::Instant::from_days(1),
                        server_send_time: clock_bound::daemon::time::Instant::from_days(1),
                        root_delay: clock_bound::daemon::time::Duration::from_micros(1),
                        root_dispersion: clock_bound::daemon::time::Duration::from_micros(1),
                        stratum: clock_bound::daemon::event::Stratum::TWO,
                    })
                    .build()
                    .unwrap(),
            )
        };
        assert!(ensure_source(&mut alg, &ntp(123), &ff_sync_params));
        assert!(!ensure_source(&mut alg, &ntp(123), &ff_sync_params));
        assert!(ensure_source(&mut alg, &ntp(1123), &ff_sync_params)); // different port: different source
        assert_eq!(alg.ntp_sources().len(), 2);
    }
}
