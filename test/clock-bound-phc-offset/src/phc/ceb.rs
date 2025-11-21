use clock_bound::daemon::time::Duration;
use std::path::PathBuf;

use thiserror::Error;

#[derive(Error, Debug, Clone, serde::Serialize)]
pub enum CebReadError {
    #[error("failed to read phc errror bound path to string: `{0}`")]
    Io(String),
    #[error("failed to parse PHC error bound value to i64: `{0}`")]
    ParseInt(String),
}

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

    pub fn read(&self) -> Result<Duration, CebReadError> {
        let contents = std::fs::read_to_string(&self.sysfs_phc_error_bound_path)
            .map_err(|e| CebReadError::Io(e.to_string()))?;
        let nanos = contents
            .trim()
            .parse::<i64>()
            .map_err(|e| CebReadError::ParseInt(e.to_string()))?;
        Ok(Duration::from_nanos(nanos.into()))
    }
}
