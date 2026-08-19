//! Tracing subscribers used within the clockbound daemon.
//!
//! Logging in Clockbound use the tracing ecosystem for logging. While
//! most of the logs are straightforward, it makes special consideration for logging
//! all clock synchronization events in a format that allows for deterministic and
//! reproducible testing of the FF Clock Sync Algorithm

use std::path::PathBuf;

use tracing::Level;
use tracing_subscriber::Registry;
use tracing_subscriber::layer::Filter;
use tracing_subscriber::{Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt};

use super::layer::StructuredLogLayer;
use super::{CLOCK_METRICS_TARGET, LogHandles, StructuredLog, is_structured_target};

/// The effective logging settings the subscriber initializes with, after merging
/// the configuration file with the command line.
#[derive(Debug, bon::Builder)]
pub struct LogSettings {
    /// Level for the stdout layer.
    level: Level,
    /// The file streams to write, or `None` to write none.
    streams: Option<FileStreams>,
}

/// Where the file streams are written, and which of them are enabled.
#[derive(Debug, bon::Builder)]
pub struct FileStreams {
    /// Directory the streams below are written to.
    directory: PathBuf,
    /// Whether the synchronization stream is written.
    synchronization: bool,
    /// Whether the ffevents stream is written.
    ffevents: bool,
}

impl FileStreams {
    /// Whether `stream` is one of the enabled ones.
    fn enables(&self, stream: StructuredLog) -> bool {
        match stream {
            StructuredLog::FFEvents => self.ffevents,
            StructuredLog::Synchronization => self.synchronization,
            // Not configurable
            StructuredLog::AlgoAnalysis => false,
        }
    }
}

/// Initialize the tracing subscriber.
///
/// Returns [`LogHandles`] for SIGHUP-triggered log file reopening.
pub fn init(settings: &LogSettings) -> LogHandles {
    let cargo_version = env!("CARGO_PKG_VERSION");
    let streams = settings.streams.as_ref();

    // Structured streams owned by the daemon. Add a stream here to wire it up
    // automatically; Each still has to be enabled by the settings to get a layer.
    let daemon_streams = [StructuredLog::FFEvents, StructuredLog::Synchronization];

    let mut structured_layers = Vec::new();
    let mut handles = Vec::new();
    let mut clock_metrics_layer = None;

    if let Some(streams) = streams {
        for stream in daemon_streams
            .into_iter()
            .filter(|stream| streams.enables(*stream))
        {
            let (layer, handle) =
                StructuredLogLayer::for_stream(&streams.directory, stream, cargo_version);
            structured_layers.push(layer);
            handles.push(handle);
        }

        let clock_metrics_writer =
            tracing_appender::rolling::hourly(&streams.directory, "clock_metrics.log");
        clock_metrics_layer = Some(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(clock_metrics_writer)
                .with_filter(filter_fn(|md| {
                    md.target().starts_with(CLOCK_METRICS_TARGET)
                })),
        );
    }

    // Logging layer for regular log events (progress, warnings) being pushed to stdout.
    let log_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout) // this is the default, just making it explicit
        .with_filter(clockbound_filter(settings.level))
        .with_filter(filter_fn(|md| !is_structured_target(md.target())));

    // `tracing_subscriber` implements `Layer` for `Vec<L>`, so the structured layers
    // compose as a single `.with(..)` entry. An empty one registers
    // `Interest::never()` for every callsite, which short-circuits the layers beneath
    // it and so silences stdout too; pass `None` instead.
    let structured_layers: Option<Vec<StructuredLogLayer>> =
        (!structured_layers.is_empty()).then_some(structured_layers);

    tracing_subscriber::registry()
        .with(log_layer)
        .with(structured_layers)
        .with(clock_metrics_layer)
        .init();

    tracing::debug!("Initialized tracing subscriber");
    if let Some(streams) = streams {
        tracing::info!(log_directory = %streams.directory.display(), "Initialized log directory");
    }

    LogHandles { handles }
}

// this pattern would be a lot cleaner with `cfg_select`, however that was only stabilized in rust this year
// Out of precaution on MSRV, not using it.
#[cfg(feature = "env-filter")]
fn clockbound_filter(level: Level) -> impl Filter<Registry> {
    use tracing_subscriber::filter::EnvFilter;
    EnvFilter::builder()
        .with_default_directive(level.into())
        .from_env_lossy()
}

#[cfg(not(feature = "env-filter"))]
fn clockbound_filter(level: Level) -> impl Filter<Registry> {
    use tracing_subscriber::filter::Targets;
    Targets::new().with_default(level)
}
