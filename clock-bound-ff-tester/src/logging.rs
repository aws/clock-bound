//! Tracing subscribers used by ff-tester.
//!
//! Running ff-tester produces distinct artifacts, dependending on how it is invoked. Replaying an
//! existing time source trace generates clock synchronization data enabling further analysis. The
//! simulator invocation does generate time source events.
//!
//! Independent from what data is emitted, the tracing ecosystem is used in all cases. This aligns
//! with the use in the ClockBound daemon, to ensure reproducible testing and data analysis.

use std::path::Path;
use tracing::Level;
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};

use clock_bound::daemon::logging::{
    ALGO_ANALYSIS_TARGET, is_structured_target, layer::StructuredLogLayer,
};

// TODO: implement logging initialization for other use cases (simulator, etc)

/// Initialize the tracing subscriber for feed-forward synchronization replay.
///
/// # Panics
/// Panics if the output directory cannot be created
pub fn init_for_synchronization_replay(log_directory: impl AsRef<Path>) {
    // Make sure that the destination directory exists
    std::fs::create_dir_all(&log_directory)
        .expect("Failed to create the algo analysis log directory");

    let (algo_analysis_layer, _algo_handle) = StructuredLogLayer::new(
        &log_directory,
        "algo_analysis.log",
        ALGO_ANALYSIS_TARGET,
        env!("CARGO_PKG_VERSION"),
    );

    // Logging layer for regular log events (progress, warnings) being pushed to stdout.
    // Verbose messages to export ffevents, synchronization and source events are filtered out.
    // Defaults to INFO.
    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout)
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(Level::INFO.into())
                .from_env_lossy(),
        )
        .with_filter(filter_fn(|md| !is_structured_target(md.target())));

    tracing_subscriber::registry()
        // Register the stdout logging layer
        .with(stdout_layer)
        // Register the synchronization algorithm layer
        .with(algo_analysis_layer)
        .init();

    tracing::info!("Initialized tracing subscriber");
}
