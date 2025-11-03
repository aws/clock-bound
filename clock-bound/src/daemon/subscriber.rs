//! Tracing subscribers used within the clockbound daemon.
//!
//! Logging in Clockbound use the tracing ecosystem for logging. While
//! most of the logs are straightforward, it makes special consideration for logging
//! all clock synchronization events in a format that allows for deterministic and
//! reproducible testing of the FF Clock Sync Algorithm

use std::path::{Path, PathBuf};

use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};

pub const PRIMER_TARGET: &str = "clock_bound::primer";

/// Initialize the tracing subscriber
///
/// This currently uses the [`tracing_subscriber::registry()`] to have 2 different paths. Clock synchronization
/// events for reproducing test cases use the target of [`PRIMER_TARGET`] to route into a separate file, while
/// the rest of the logs go through the default logger (presently writes to stdout)
pub fn init(log_directory: impl AsRef<Path>) {
    let primer_writer = tracing_appender::rolling::never(&log_directory, "primer.log");
    let primer_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(primer_writer)
        .with_filter(filter_fn(|md| md.target().starts_with(PRIMER_TARGET)));

    let log_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout) // this is the default, just making it explicit
        .with_filter(EnvFilter::from_default_env())
        .with_filter(filter_fn(|md| !md.target().starts_with(PRIMER_TARGET)));

    tracing_subscriber::registry()
        // this is the default logging layer
        .with(log_layer)
        // and this is the primer reproducibility layer
        .with(primer_layer)
        .init();

    tracing::info!("Initialized tracing subscriber");
    let primer_log_file = PathBuf::from(log_directory.as_ref()).join("primer.log");
    tracing::info!(primer_log_file = %primer_log_file.display(), "Primer log file");
}
