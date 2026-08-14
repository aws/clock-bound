//! ClockBound daemon
use std::path::PathBuf;

use clap::Parser;
use clock_bound::daemon::{
    Daemon, logging::subscriber, logging::synchronization::log_daemon_started,
};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

#[derive(Debug, Parser)]
struct Args {
    #[clap(long, default_value = "/var/log/clockbound")]
    log_dir: PathBuf,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let args = Args::parse();
    let log_handles = subscriber::init(&args.log_dir);
    log_daemon_started();
    let cancellation_token = CancellationToken::new();
    let d = Daemon::construct(cancellation_token.clone()).await;
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
                    Ok(()) => info!("daemon exited gracefully."),
                    Err(e) => warn!(?e, "daemon exited ungracefully.")
                }
                break;
            }
            _ = sighup.recv() => {
                info!("SIGHUP received. Reopening log files.");
                log_handles.reopen_all();
            }
            res = &mut daemon_handle => {
                error!(?res, "daemon exited unexpectedly.");
                break;
            }
        }
    }
}
