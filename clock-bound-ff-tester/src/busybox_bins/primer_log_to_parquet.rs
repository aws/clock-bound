use std::path::PathBuf;

use crate::repro::{self, ReproEvent};
use clap::Parser;
use polars::prelude::*;

#[derive(Parser, Clone, Debug)]
/// Convert a primer/ffevents log file to a newline delimited json
pub struct Cli {
    /// The logfile to transform
    #[arg(short, long)]
    logfile: PathBuf,

    /// The output transformed file
    #[arg(short, long)]
    output: PathBuf,
}

pub fn main(args: Cli) -> anyhow::Result<()> {
    let repro_events = repro::repro_events_from_log_file(&args.logfile)?;
    println!("Parsed events. Converting into a dataframe");
    let mut df = ReproEvent::into_dataframe(repro_events).unwrap();
    println!("Dataframe created");
    println!("Writing to file");
    let file = std::fs::File::create_new(&args.output)?;
    let bytes = ParquetWriter::new(file).finish(&mut df)?;
    println!("Wrote {bytes} bytes to {}", args.output.display());
    Ok(())
}
