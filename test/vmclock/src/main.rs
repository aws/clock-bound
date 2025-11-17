//! VMClock test executable
//!
//! This executable tests the vmclock runner. It tests that the runner is able to access, and load
//! the vmclock shared memory file.

use clock_bound::daemon::io::SourceIO;
use clock_bound::daemon::io::ntp::DaemonInfo;
use clock_bound::daemon::selected_clock::SelectedClockSource;
use clock_bound::vmclock::shm::VMCLOCK_SHM_DEFAULT_PATH;

use rand::{RngCore, rng};
use std::sync::Arc;
use tokio::time::Duration;
use tracing_subscriber::EnvFilter;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let daemon_info = DaemonInfo {
        major_version: 2,
        minor_version: 100,
        startup_id: rng().next_u64(),
    };

    let selected_clock = Arc::new(SelectedClockSource::default());
    let mut sourceio = SourceIO::construct(selected_clock.clone(), daemon_info);

    println!("Attempting to create vmclock runner using {VMCLOCK_SHM_DEFAULT_PATH:?} file.");
    sourceio.create_vmclock(VMCLOCK_SHM_DEFAULT_PATH).await;
    sourceio.spawn_all();

    // Allow the runner to poll the shared memory file for a while.
    tokio::time::sleep(Duration::from_secs(3)).await;

    println!("VMClock running initiation and polling test PASSED.");
}
