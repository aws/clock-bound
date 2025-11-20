//! Replay a repro logfile against the current `ClockSyncAlgorithm` and see how it goes
//!
//! Prints the new outputs into another log-directory

use std::{path::PathBuf, sync::Arc};

use clap::Parser;
use clock_bound::daemon::{
    self,
    clock_sync_algorithm::{ClockSyncAlgorithm, Selector, source},
    selected_clock::SelectedClockSource,
};
use clock_bound_ff_tester::{
    events::Scenario,
    repro::{self, routable_from_tester_event},
    time::Skew,
};

#[derive(Parser, Debug)]
/// Replay a scenario from a logfile back into the `ClockSyncAlgorithm`
///
/// Outputs the results from this run into `--output-dir` with the same
/// format as the input
struct Args {
    /// The logfile to replay
    #[arg(short, long)]
    logfile: PathBuf,

    /// The output log directory
    #[arg(short, long)]
    output_dir: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let max_dispersion = Skew::from_ppm(15.0);
    let mut alg = ClockSyncAlgorithm::builder()
        .link_local(source::LinkLocal::new(max_dispersion))
        .ntp_sources(vec![])
        .phc(source::Phc::new("/dev/ptp0".into(), max_dispersion))
        .selected_clock(Arc::new(SelectedClockSource::default()))
        .selector(Selector::new(max_dispersion))
        .build();

    let scenario = repro::scenario_from_log_file(&args.logfile)?;

    daemon::subscriber::init(args.output_dir);

    let Scenario::V1(scenario) = scenario.0;
    let events = scenario.events;

    for event in events {
        let routable = routable_from_tester_event(event);
        alg.feed(routable);
    }

    Ok(())
}
