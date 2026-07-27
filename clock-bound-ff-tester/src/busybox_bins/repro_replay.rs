//! Replay a repro logfile against the current `ClockSyncAlgorithm` and see how it goes
//!
//! Prints the new outputs into another log-directory

use std::{path::PathBuf, sync::Arc};

use crate::{
    repro::{self, ReproEvent},
    time::Skew,
};
use clap::Parser;
use clock_bound::daemon::{
    self,
    clock_sync_algorithm::{ClockSyncAlgorithm, Selector, source},
    selected_clock::SelectedClockSource,
};

#[derive(Parser, Clone, Debug)]
/// Replay a scenario from a logfile back into the `ClockSyncAlgorithm`
///
/// Outputs the results from this run into `--output-dir` with the same
/// format as the input
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

    let repro_events = repro::repro_events_from_log_file(&args.logfile)?;

    // needed to output new clock parameters
    daemon::subscriber::init(args.output_dir);

    for event in repro_events {
        match event {
            ReproEvent::Init => tracing::warn!("Received init."), // TODO: Better breakdown into scenarios
            ReproEvent::Disruption => {
                tracing::warn!("Received disruption.");
                alg.handle_disruption();
            }
            ReproEvent::Feed { event, .. } => {
                // unused result since output log directory will contain this
                let _ = alg.feed(event);
            }
        }
    }

    Ok(())
}
