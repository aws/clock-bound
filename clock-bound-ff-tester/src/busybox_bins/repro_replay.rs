//! Replay a repro logfile against the current `ClockSyncAlgorithm` and see how it goes
//!
//! Prints the new outputs into another log-directory

use std::{path::PathBuf, sync::Arc};

use crate::{
    logging,
    repro::{self, ReproEvent},
    time::Skew,
};
use clap::Parser;
use clock_bound::daemon::{
    clock_sync_algorithm::{ClockSyncAlgorithm, Selector, source},
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

pub fn main(args: Cli) -> anyhow::Result<()> {
    let max_dispersion = Skew::from_ppm(15.0);
    let mut alg = ClockSyncAlgorithm::builder()
        .amazon_time_sync(source::AmazonTimeSync::new(max_dispersion))
        .ntp_sources(vec![])
        .phc(source::Phc::new("/dev/ptp0".into(), max_dispersion))
        .selected_clock(Arc::new(SelectedClockSource::default()))
        .selector(Selector::new(max_dispersion))
        .build();

    // Needed to output updated clock estimates and internal algorithm calculations to their
    // respective log files.
    logging::init_for_synchronization_replay(args.output_dir);

    let mut repro_events = repro::repro_events_from_log_file(&args.logfile)?;

    // Stream events straight from the log: memory usage is bounded by the longest
    // line, not the file size. Parse errors abort the replay, as a partially
    // replayed log would produce misleading analysis data.
    let (mut feeds, mut inits, mut disruptions) = (0usize, 0usize, 0usize);
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
        ordering = %repro_events.order_summary(),
        "replay complete"
    );

    Ok(())
}
