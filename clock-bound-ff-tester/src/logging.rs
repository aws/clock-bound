//! Tracing subscribers used by ff-tester.
//!
//! Running ff-tester produces distinct artifacts, dependending on how it is invoked. Replaying an
//! existing time source trace generates clock synchronization data enabling further analysis. The
//! simulator invocation does generate time source events.
//!
//! Independent from what data is emitted, the tracing ecosystem is used in all cases. This aligns
//! with the use in the ClockBound daemon, to ensure reproducible testing and data analysis.

use std::fmt::Write;
use std::path::Path;
use tracing::Level;
use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, layer::SubscriberExt, util::SubscriberInitExt,
};

use clock_bound::daemon::logging::{
    LogHandles, StructuredLog, is_structured_target,
    layer::{StructuredLogLayer, TimestampSource},
};

// TODO: implement logging initialization for other use cases (simulator, etc)

/// Create a new directory to store output logs.
///
/// The daemon appends event to output log files if they already exist. This is never what we want
/// in the current model where the input is a single `FFevents` log file, and would likely be
/// confusing any post-processing / data analysis tool.
///
/// Exit with an error message if the output directory already exists.
///
/// # Panics
/// Panics if the output directory cannot be confirmed to exist
/// Panics if the output directory cannot be created
fn create_log_directory(log_directory: impl AsRef<Path>) {
    let maybe_exists = match std::fs::exists(&log_directory) {
        Ok(false) => false,
        Ok(true) => {
            eprintln!("Error: Output log directory already exists");
            true
        }
        Err(e) => {
            eprintln!("Failed to confirm if the output log directory already exists: {e}");
            true
        }
    };

    if maybe_exists {
        std::process::exit(1);
    }

    // Can now create an empty directory to ensure we can write to
    if let Err(e) = std::fs::create_dir_all(&log_directory) {
        eprintln!("Failed to create the output log directory: {e}");
        std::process::exit(1);
    }
}

/// Initialize the tracing subscriber for feed-forward synchronization replay.
///
/// Create output directory where the logs of events and ffsync algorithm internal metrics will be
/// stored for further analysis.
///
/// `version_suffix` is appended to the version string burnt into every log line; the replay
/// passes `+params-<hash>` when it runs with non-default algorithm parameters (see
/// [`crate::ffsync_params`]), and nothing when it runs with the defaults, so that a default
/// replay carries exactly the version string the daemon carries.
///
/// Returns the handles to the two streams; the caller flushes them when the replay is done
/// ([`LogHandles::flush_all`]): the layers live in the global subscriber and are never dropped.
pub fn init_for_synchronization_replay(
    log_directory: impl AsRef<Path>,
    version_suffix: Option<&str>,
) -> LogHandles {
    // The StructuredLogLayer attempts to create directory, so need to assert whether the directory
    // already exists
    // create a *new* directory where to write log files to
    create_log_directory(&log_directory);

    // Burn the git commit into the logs. This helps ensure we know which exact version of the
    // algorithm is being used, and later on, analyzed for accuracy and performance. Also mark mark
    // uncommitted changes if any.
    let mut version = String::from(env!("CARGO_PKG_VERSION"));
    if let Some(s) = option_env!("GIT_COMMIT_HASH_SHORT") {
        write!(version, "+git-{s}").unwrap();
    }
    if let Some(s) = option_env!("GIT_REPO_STATUS")
        && s == "dirty"
    {
        write!(version, "-dirty").unwrap();
    }
    if let Some(suffix) = version_suffix {
        version.push_str(suffix);
    }
    let static_version: &'static str = version.leak();

    // Create the various data streams.
    // Note that `FFEvents` log is (re-)generated which is helpful for any schema migration but also to
    // compare with original run and assert differences in ffsync algorithm versions.
    //
    // A replay is a pure function of (trace, algorithm, parameters), and given the same input, the
    // output must be identical across runs and hosts. Although the daemon stamps every log line
    // with the wall clock, this is meaningless for a replay. Hence the logging layer is initialized
    // with a timestamp fixed at the epoch, so that the same replay yields the same output.
    // The algo_analysis records are joined to the feed on `seq` and carry no timestamp at all.
    let mut structured_layers = Vec::new();
    let mut handles = Vec::new();
    let data_streams = [
        (StructuredLog::AlgoAnalysis, TimestampSource::None),
        (StructuredLog::FFEvents, TimestampSource::Fixed),
    ];
    for (stream, timestamp_source) in data_streams {
        let (layer, handle) = StructuredLogLayer::for_stream(
            &log_directory,
            stream,
            static_version,
            timestamp_source,
        );
        structured_layers.push(layer);
        handles.push(handle);
    }

    // Logging layer for regular log events (progress, warnings) being pushed to stdout.
    // Verbose messages to export FFevents, synchronization and source events are filtered out.
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
        // Register the structured layers
        .with(structured_layers)
        .init();

    tracing::info!("Initialized tracing subscriber");
    LogHandles::new(handles)
}
