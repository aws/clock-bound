//! ClockBound daemon
use std::path::{Path, PathBuf};
use std::process;

use clap::Parser;
use clock_bound::daemon::{
    Daemon,
    config::{Config, LoggingConfig, SourcesConfig},
    logging::subscriber::{self, FileStreams, LogSettings},
    logging::synchronization::log_daemon_started,
};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing::{Level, debug, error, info, warn};

/// Exit code for a bad configuration, distinct from a runtime crash.
const CONFIG_ERROR_EXIT: u8 = 2;

#[derive(Debug, Parser)]
struct Args {
    /// Path to the configuration file. Without one the daemon runs on defaults
    #[clap(short, long)]
    configuration: Option<PathBuf>,

    /// Raise the stdout level from `info` to `debug`.
    #[clap(short, long)]
    verbose: bool,

    /// Load, parse, and validate the configuration, report the outcome, and exit without
    /// starting the daemon.
    #[clap(long, requires = "configuration")]
    check_configuration: bool,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let args = Args::parse();

    // Load and validate the configuration before anything is set up, so a bad one
    // reports its cause and stops here.
    let config = match Config::load(args.configuration.clone()) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            process::exit(CONFIG_ERROR_EXIT.into());
        }
    };

    // `--check-configuration`: Load, parse, and validate the provided configuration
    // file. Report the outcome and exit.
    if args.check_configuration {
        let path = args
            .configuration
            .expect("--check-configuration requires -c");
        println!("{} is valid", path.display());
        return;
    }

    let (logging, sources) = config.into_parts();

    let streams = logging.log_directory().map(|directory| {
        FileStreams::builder()
            .directory(directory.to_path_buf())
            .synchronization(logging.synchronization().is_enabled())
            .ffevents(logging.ffevents().is_enabled())
            .build()
    });
    let log_handles = subscriber::init(
        &LogSettings::builder()
            .level(if args.verbose {
                Level::DEBUG
            } else {
                Level::INFO
            })
            .maybe_streams(streams)
            .build(),
    );

    log_configuration(args.configuration.as_deref(), &logging, &sources);
    let cargo_version = env!("CARGO_PKG_VERSION");
    info!("ClockBound started. Version {cargo_version}.");
    log_daemon_started();

    let cancellation_token = CancellationToken::new();
    let d = Daemon::construct(sources, cancellation_token.clone()).await;
    let d = Box::new(d);
    let mut daemon_handle = tokio::spawn(async move { d.run().await });

    let mut sigint = signal(SignalKind::interrupt()).expect("failed to create SIGINT listener.");
    let mut sighup = signal(SignalKind::hangup()).expect("failed to create SIGHUP listener.");

    loop {
        tokio::select! {
            _ = sigint.recv() => {
                info!("SIGINT received. Starting graceful shutdown of daemon.");
                cancellation_token.cancel();
                match daemon_handle.await {
                    Ok(()) => info!("Daemon exited gracefully."),
                    Err(e) => warn!(?e, "Daemon exited ungracefully.")
                }
                break;
            }
            _ = sighup.recv() => {
                info!("SIGHUP received. Reopening log files.");
                log_handles.reopen_all();
            }
            res = &mut daemon_handle => {
                error!(?res, "Daemon exited unexpectedly.");
                break;
            }
        }
    }
    log_handles.flush_all();
}

fn log_configuration(
    configuration: Option<&Path>,
    logging: &LoggingConfig,
    sources: &SourcesConfig,
) {
    if let Some(path) = configuration {
        info!(configuration = %path.display(), "Loaded configuration");
    } else {
        info!("No configuration file supplied, running on defaults");
    }

    let log_directory = logging
        .log_directory()
        .map_or_else(|| "none".to_owned(), |path| path.display().to_string());
    debug!(
        %log_directory,
        synchronization = logging.synchronization().is_enabled(),
        ffevents = logging.ffevents().is_enabled(),
        ntp = ?sources.ntp(),
        "Effective configuration"
    );
}
