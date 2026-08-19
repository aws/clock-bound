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

use super::layer::StructuredLogLayer;
use super::{CLOCK_METRICS_TARGET, LogHandles, StructuredLog, is_structured_target};

/// Initialize the tracing subscriber.
///
/// Returns [`LogHandles`] for SIGHUP-triggered log file reopening.
pub fn init(log_directory: impl AsRef<Path>) -> LogHandles {
    let cargo_version = env!("CARGO_PKG_VERSION");

    // Structured streams owned by the daemon. Add a stream here to wire it up
    // automatically; omit one to disable it (its handle is simply never pushed).
    let daemon_streams = [StructuredLog::FFEvents, StructuredLog::Synchronization];

    let mut structured_layers = Vec::new();
    let mut handles = Vec::new();
    for stream in daemon_streams {
        let (layer, handle) = StructuredLogLayer::for_stream(&log_directory, stream, cargo_version);
        structured_layers.push(layer);
        handles.push(handle);
    }

    let clock_metrics_writer =
        tracing_appender::rolling::hourly(&log_directory, "clock_metrics.log");
    let clock_metrics_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_writer(clock_metrics_writer)
        .with_filter(filter_fn(|md| {
            md.target().starts_with(CLOCK_METRICS_TARGET)
        }));

    // Logging layer for regular log events (progress, warnings) being pushed to stdout.
    // Verbose messages to export ffevents, synchronization and source events are filtered out.
    // Defaults to INFO.
    let log_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout) // this is the default, just making it explicit
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(Level::INFO.into())
                .from_env_lossy(),
        )
        .with_filter(filter_fn(|md| !is_structured_target(md.target())));

    // `tracing_subscriber` implements `Layer` for `Vec<L>`, so the structured
    // layers compose as a single `.with(..)` entry.
    tracing_subscriber::registry()
        .with(log_layer)
        .with(structured_layers)
        .with(clock_metrics_layer)
        .init();

    tracing::info!("Initialized tracing subscriber");
    tracing::info!(log_directory = %log_directory.as_ref().display(), "Initialized log directory");

    LogHandles { handles }
}
