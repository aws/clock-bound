//! Tracing subscribers used within the clockbound daemon.
//!
//! Logging in Clockbound use the tracing ecosystem for logging. While
//! most of the logs are straightforward, it makes special consideration for logging
//! all clock synchronization events in a format that allows for deterministic and
//! reproducible testing of the FF Clock Sync Algorithm

use std::path::Path;

use tracing::Level;
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};

use super::{CLOCK_METRICS_TARGET, PRIMER_TARGET};

/// Initialize the tracing subscriber
///
/// This currently uses the [`tracing_subscriber::registry()`] to have 2 different paths. Clock synchronization
/// events for reproducing test cases use the target of [`PRIMER_TARGET`] to route into a separate file, while
/// the rest of the logs go through the default logger (presently writes to stdout)
pub fn init(log_directory: impl AsRef<Path>) {
    let primer_writer = tracing_appender::rolling::hourly(&log_directory, "primer.log");
    let primer_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(primer_writer)
        .with_filter(filter_fn(|md| md.target().starts_with(PRIMER_TARGET)));

    let clock_metrics_writer =
        tracing_appender::rolling::hourly(&log_directory, "clock_metrics.log");
    let clock_metrics_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(clock_metrics_writer)
        .with_filter(filter_fn(|md| {
            md.target().starts_with(CLOCK_METRICS_TARGET)
        }));

    let log_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout) // this is the default, just making it explicit
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(Level::INFO.into())
                .from_env_lossy(),
        )
        .with_filter(filter_fn(|md| !md.target().starts_with(PRIMER_TARGET)))
        .with_filter(filter_fn(|md| {
            !md.target().starts_with(CLOCK_METRICS_TARGET)
        }));

    tracing_subscriber::registry()
        // this is the default logging layer
        .with(log_layer)
        // and this is the primer reproducibility layer
        .with(primer_layer)
        // and this is the clock metrics layer
        .with(clock_metrics_layer)
        .init();

    tracing::info!("Initialized tracing subscriber");
    tracing::info!(primer_log_file = %log_directory.as_ref().display(), "Initialized log directory");
}
