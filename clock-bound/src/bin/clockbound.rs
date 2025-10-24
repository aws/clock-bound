//! ClockBound daemon
use clock_bound::daemon::Daemon;

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    tracing_subscriber::fmt::init();
    let mut d = Daemon::construct().await;
    tokio::spawn(async move { d.run().await }).await.unwrap();
}
