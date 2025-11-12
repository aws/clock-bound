//! ClockBound daemon
use std::path::PathBuf;

use clap::Parser;
use clock_bound::daemon::{Daemon, subscriber};

#[derive(Debug, Parser)]
struct Args {
    #[clap(long, default_value = "clockbound")]
    log_dir: PathBuf,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let args = Args::parse();
    subscriber::init(&args.log_dir);
    let d = Daemon::construct().await;
    let d = Box::new(d);
    tokio::spawn(async move { d.run().await }).await.unwrap();
}
