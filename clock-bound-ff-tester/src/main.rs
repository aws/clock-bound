//! Main entry point for crate binary
//!
//! Using a busybox style binary

use clap::{Parser, Subcommand};
use clock_bound_ff_tester::busybox_bins as bins;

#[derive(Clone, Debug, Parser)]
/// CLI for ff-tester
pub struct Cli {
    #[command(subcommand)]
    /// Subcommand for this CLI
    pub command: Commands,
}

#[derive(Clone, Debug, Subcommand)]
pub enum Commands {
    FixLogOrder(bins::fix_log_order::Cli),
    OscillatorGenerator(bins::oscillator_generator::Cli),
    ReproReplay(bins::repro_replay::Cli),
    ScenarioGenerator(bins::scenario_generator::Cli),
    SimAmazonTimeSync,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::FixLogOrder(cli) => bins::fix_log_order::main(cli)?,
        Commands::OscillatorGenerator(cli) => bins::oscillator_generator::main(cli)?,
        Commands::ReproReplay(cli) => bins::repro_replay::main(cli)?,
        Commands::ScenarioGenerator(cli) => bins::scenario_generator::main(cli)?,
        Commands::SimAmazonTimeSync => bins::sim_amazon_time_sync::main(),
    }

    Ok(())
}
