// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only

//! ClockBound Daemon
//!
//! This crate implements the ClockBound daemon

use clap::Parser;
use tracing::{info, warn, Level};

use clock_bound_d::thread_manager::run;
use clock_bound_d::{get_error_bound_sysfs_path, refid_to_u32, PhcInfo};

// XXX: A default value of 1ppm is VERY wrong for common XO specs these days.
// Sadly we have to align default value with chrony.
pub const DEFAULT_MAX_DRIFT_RATE_PPB: u32 = 1000;

#[derive(Parser, Debug)]
#[command(author, name = "clockbound", version, about, long_about = None)]
struct Cli {
    /// Set the maximum drift rate of the underlying oscillator in ppm (default 1ppm).
    /// Chrony `maxclockerror` configuration should be set to match this value.
    #[arg(short, long)]
    max_drift_rate: Option<u32>,

    /// Emit structured log messages. Default to human readable.
    #[arg(short, long)]
    json_output: bool,

    /// The PHC reference ID from Chronyd (generally, this is PHC0).
    /// Required for configuring ClockBound to sync to PHC.
    #[arg(short = 'r', long, requires = "phc_interface", value_parser = refid_to_u32)]
    phc_ref_id: Option<u32>,

    /// The network interface that the ENA driver PHC exists on (e.g. eth0).
    /// Required for configuring ClockBound to sync to PHC.
    #[arg(short = 'i', long, requires = "phc_ref_id")]
    phc_interface: Option<String>,
}

// ClockBound application entry point.
fn main() -> Result<(), String> {
    let args = Cli::parse();

    // Configure the fields emitted in log messages
    let format = tracing_subscriber::fmt::format()
        .with_level(true)
        .with_target(false)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true);

    // Create a `fmt` subscriber that uses the event format.
    // Enable all levels up to DEBUG here, but remember that the crate is configured to strip out
    // DEBUG level for release builds. The builder also provide the option to emit human readable
    // or JSON structured logs.
    let builder = tracing_subscriber::fmt().with_max_level(Level::DEBUG);

    if args.json_output {
        builder
            .event_format(format.json().flatten_event(true))
            .init();
    } else {
        builder.event_format(format).init();
    };

    // Log a message confirming the daemon is starting. Always useful if in a reboot loop.
    info!("ClockBound daemon is starting");

    // TODO: should introduce a config object to gather options on the CLI etc.
    let max_drift_ppb = match args.max_drift_rate {
        Some(rate) => rate * 1000,
        None => {
            warn!("Using the default max drift rate of 1PPM, which is likely wrong. \
                  Update chrony configuration and clockbound to a value that matches your hardware.");
            DEFAULT_MAX_DRIFT_RATE_PPB
        }
    };

    let phc_info = match (args.phc_interface, args.phc_ref_id) {
        (Some(interface), Some(refid)) => {
            let sysfs_error_bound_path = get_error_bound_sysfs_path(&interface)?;
            Some(PhcInfo {
                refid,
                sysfs_error_bound_path,
            })
        }
        _ => None,
    };
    run(max_drift_ppb, phc_info);
    Ok(())
}
