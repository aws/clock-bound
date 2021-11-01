// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: GPL-2.0-only
use clap::{value_t, App, Arg};
use clock_bound_d::run;
use syslog::Error;

// Constants that reference package information from Cargo.toml
const VERSION: &'static str = env!("CARGO_PKG_VERSION");
const NAME: &'static str = env!("CARGO_PKG_NAME");
const AUTHORS: &'static str = env!("CARGO_PKG_AUTHORS");
const DESCRIPTION: &'static str = env!("CARGO_PKG_DESCRIPTION");

pub const DEFAULT_MAX_CLOCK_ERROR: f64 = 1.0; // 1ppm, same value as what chronyd is hard-coded to

// ClockBoundD application entry point.
fn main() -> Result<(), Error> {
    // Create CLI options using Cargo.toml package information for reference
    let matches = App::new(NAME)
        .version(VERSION)
        .author(AUTHORS)
        .about(DESCRIPTION)
        .arg(Arg::with_name("level")
            .short("L")
            .long("level")
            .takes_value(true)
            .possible_values(&["0", "1", "2", "3", "4"])
            .help("Set the minimum log level of messages written to syslog. The available levels are: 0 (Trace), 1 (Debug), 2 (Info), 3 (Warning), 4 (Error). The default value is 2 (Info)."))
        .arg(Arg::with_name("max_clock_error")
            .short("e")
            .long("max_clock_error")
            .takes_value(true)
            .help("Set the max clock error in ppm. This is the assumed maximum frequency error that a system clock can gain between updates. This should be set to the same value as maxclockerror in chrony's configuration. Default value is 1 ppm."))
        .get_matches();

    // Validate max_clock_error is a float. Otherwise, use the default value.
    // Clap will log an error and exit if an invalid argument is provided.
    let max_clock_error = if matches.is_present("max_clock_error") {
        value_t!(matches.value_of("max_clock_error"), f64).unwrap_or_else(|e| e.exit())
    } else {
        DEFAULT_MAX_CLOCK_ERROR
    };

    // Default minimum log level is Info
    let mut log_level = log::LevelFilter::Info;
    if matches.is_present("level") {
        log_level = match matches.value_of("level").unwrap() {
            "0" => log::LevelFilter::Trace,
            "1" => log::LevelFilter::Debug,
            "2" => log::LevelFilter::Info,
            "3" => log::LevelFilter::Warn,
            "4" => log::LevelFilter::Error,
            _ => log::LevelFilter::Info,
        };
    }

    // Setup syslog format to match RFC 3164
    // Logs are output to the syslog /var/log/daemon.log
    let formatter = syslog::Formatter3164 {
        facility: syslog::Facility::LOG_DAEMON,
        hostname: None,
        process: "clockboundd".into(),
        pid: std::process::id() as i32,
    };

    let logger = syslog::unix(formatter).expect("could not connect to syslog");
    log::set_boxed_logger(Box::new(syslog::BasicLogger::new(logger)))
        .map(|()| log::set_max_level(log_level))
        .unwrap();

    run(max_clock_error);
    Ok(())
}
