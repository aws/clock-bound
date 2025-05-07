//! ClockBound Daemon
//!
//! This crate implements the ClockBound daemon

use std::str::FromStr;
use std::sync::atomic::Ordering;

use clap::Parser;
use tracing::{error, info, warn, Level};

use clock_bound_d::run;
use clock_bound_d::signal::register_signal_callback;
use clock_bound_d::{
    get_error_bound_sysfs_path, refid_to_u32, ClockErrorBoundSource, PhcInfo,
    FORCE_DISRUPTION_PENDING, FORCE_DISRUPTION_STATE,
};

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

    /// Run without support for clock disruptions. Default to false.
    #[arg(short, long)]
    disable_clock_disruption_support: bool,

    /// The PHC reference ID from Chronyd (generally, this is PHC0).
    /// Required for configuring ClockBound to sync to PHC.
    #[arg(short = 'r', long, requires = "phc_interface", value_parser = refid_to_u32)]
    phc_ref_id: Option<u32>,

    /// The network interface that the ENA driver PHC exists on (e.g. eth0).
    /// Required for configuring ClockBound to sync to PHC.
    #[arg(short = 'i', long, requires = "phc_ref_id")]
    phc_interface: Option<String>,

    /// Clock Error Bound source.
    ///
    /// Valid values are: 'chrony', 'vmclock'.
    ///
    /// Selecting `vmclock` will cause us to use the Hypervisor-provided device node
    /// for determining the Clock Error Bound.
    ///
    /// By default, if this argument is not provided, then
    /// Clockbound daemon will default to using Chrony.
    #[arg(long)]
    clock_error_bound_source: Option<String>,
}

/// SIGUSR1 signal handler to force a clock disruption event.
/// This handler is primarily here for testing the clock disruption functionality in isolation.
fn on_sigusr1() {
    let state = FORCE_DISRUPTION_STATE.load(Ordering::SeqCst);
    if !state {
        info!("Received SIGUSR1 signal. Setting forced clock disruption to true.");
        FORCE_DISRUPTION_STATE.store(true, Ordering::SeqCst);
        FORCE_DISRUPTION_PENDING.store(true, Ordering::SeqCst);
    } else {
        info!("Received SIGUSR1 signal. Forced clock disruption is already true.");
    }
}

/// SIGUSR1 signal handler when clock disruption support is disabled.
fn on_sigusr1_ignored() {
    warn!("Ignoring received SIGUSR1 signal.");
}

/// SIGUSR2 signal handler to undo a force clock disruption event.
/// This handler is primarily here for testing the clock disruption functionality in isolation.
fn on_sigusr2() {
    let state = FORCE_DISRUPTION_STATE.load(Ordering::SeqCst);
    if state {
        info!("Received SIGUSR2 signal. Setting forced clock disruption to false.");
        FORCE_DISRUPTION_STATE.store(false, Ordering::SeqCst);
        FORCE_DISRUPTION_PENDING.store(true, Ordering::SeqCst);
    } else {
        info!("Received SIGUSR2 signal. Forced clock disruption is already false.");
    }
}

/// SIGUSR2 signal handler when clock disruption support is disabled.
fn on_sigusr2_ignored() {
    warn!("Ignoring received SIGUSR2 signal.");
}

// ClockBound application entry point.
fn main() -> anyhow::Result<()> {
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

    // Register callbacks on UNIX signals
    let sigusr1_callback = if args.disable_clock_disruption_support {
        on_sigusr1_ignored
    } else {
        on_sigusr1
    };
    let sigusr2_callback = if args.disable_clock_disruption_support {
        on_sigusr2_ignored
    } else {
        on_sigusr2
    };
    if let Err(e) = register_signal_callback(nix::sys::signal::SIGUSR1, sigusr1_callback) {
        error!("Failed to register callback on SIGUSR1 signal [{:?}]", e);
        return Err(e.into());
    }
    if let Err(e) = register_signal_callback(nix::sys::signal::SIGUSR2, sigusr2_callback) {
        error!("Failed to register callback on SIGUSR2 signal [{:?}]", e);
        return Err(e.into());
    }

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

    let clock_error_bound_source: ClockErrorBoundSource = match args.clock_error_bound_source {
        Some(source_str) => match ClockErrorBoundSource::from_str(&source_str) {
            Ok(v) => v,
            Err(_) => {
                let err_msg = format!("Unsupported ClockErrorBoundSource: {:?}", source_str);
                error!(err_msg);
                anyhow::bail!(err_msg);
            }
        },
        None => ClockErrorBoundSource::Chrony,
    };
    info!("ClockErrorBoundSource: {:?}", clock_error_bound_source);

    if args.disable_clock_disruption_support {
        warn!("Support for clock disruption is explicitly disabled");
    }

    run(
        max_drift_ppb,
        phc_info,
        clock_error_bound_source,
        !args.disable_clock_disruption_support,
    );
    Ok(())
}
