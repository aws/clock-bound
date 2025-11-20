use anyhow::{Context, Result};
use clock_bound::daemon::time::Duration;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct PhcClockErrorBoundReader {
    sysfs_phc_error_bound_path: PathBuf,
}

impl PhcClockErrorBoundReader {
    pub fn new(phc_clock_error_bound_path: PathBuf) -> Self {
        Self {
            sysfs_phc_error_bound_path: phc_clock_error_bound_path,
        }
    }

    pub fn read(&self) -> Result<Duration> {
        let contents = std::fs::read_to_string(&self.sysfs_phc_error_bound_path)
            .context("failed to read phc errror bound path to string")?;
        let nanos = contents
            .trim()
            .parse::<i64>()
            .context("failed to parse PHC error bound value to i64")?;
        Ok(Duration::from_nanos(nanos.into()))
    }
}
