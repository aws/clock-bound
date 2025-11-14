//! ClockBound daemon
use std::path::PathBuf;

use clap::Parser;
use clock_bound::daemon::{Daemon, subscriber};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

#[derive(Debug, Parser)]
struct Args {
    #[clap(long, default_value = "clockbound")]
    log_dir: PathBuf,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let args = Args::parse();
    subscriber::init(&args.log_dir);
    let cancellation_token = CancellationToken::new();
    let d = Daemon::construct(cancellation_token.clone()).await;
    let d = Box::new(d);
    let mut daemon_handle = tokio::spawn(async move { d.run().await });

    let mut sigint = signal(SignalKind::interrupt()).expect("failed to create SIGINT listener.");
    tokio::select! {
        _ = sigint.recv() => {
            info!("SIGINT receieved. Starting graceful shutdown of daemon.");
            cancellation_token.cancel();
            match daemon_handle.await {
                Ok(()) => info!("daemon exited gracefully."),
                Err(e) => warn!(?e, "daemon exited ungracefully.")
            }
        },
        res = &mut daemon_handle => error!(?res, "daemon exited unexpectedly."),
    }
}
