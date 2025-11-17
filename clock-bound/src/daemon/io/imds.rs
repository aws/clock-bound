//! Struct holding instance type meta data
use core::str;

use std::time::Duration;
use thiserror::Error;
use tokio::time::timeout;

#[derive(Debug, Error)]
pub enum InstanceTypeError {
    #[error("Failed to parse into json.")]
    Serde(#[from] serde_json::Error),
    #[error("Reqwest error")]
    Reqwest(#[from] reqwest::Error),
    #[error("Failed to parse bytes as utf8 string.")]
    Utf8(#[from] std::str::Utf8Error),
    #[error("Failed to get token.")]
    ReceiveToken,
    #[error("Failed to get instance meta data.")]
    ReceiveImds,
    #[error("IMDS request timed out.")]
    Timeout(#[from] tokio::time::error::Elapsed),
}

/// Instance type meta data used to determine instance capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceType(pub String);

impl InstanceType {
    // Instance metadata service
    const IMDS_ORIGIN: &'static str = "http://169.254.169.254";
    const IMDS_TIMEOUT_DURATION: Duration = Duration::from_secs(1);

    #[cfg(test)]
    pub async fn get_from_imds() -> Result<Self, InstanceTypeError> {
        // Setting InstanceType explicitly to prevent unit tests from attempting to reach out to
        // imds.
        Ok(InstanceType("m7i.xlarge".to_string()))
    }

    /// Request and parse info using the [IMDSv2 API]
    ///
    /// [IMDSv2 API]: https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configuring-instance-metadata-service.html
    #[cfg(not(test))]
    pub async fn get_from_imds() -> Result<Self, InstanceTypeError> {
        let client = reqwest::Client::new();

        // get session token
        let response = timeout(
            Self::IMDS_TIMEOUT_DURATION,
            client
                .put(format!("{}/latest/api/token", Self::IMDS_ORIGIN))
                .header("X-aws-ec2-metadata-token-ttl-seconds", "21600")
                .send(),
        )
        .await??;

        if !response.status().is_success() {
            return Err(InstanceTypeError::ReceiveToken);
        }
        let token = response.bytes().await?;

        // get actual metadata
        let response = timeout(
            Self::IMDS_TIMEOUT_DURATION,
            client
                .get(format!(
                    "{}/latest/meta-data/instance-type",
                    Self::IMDS_ORIGIN
                ))
                .header("X-aws-ec2-metadata-token", &*token)
                .send(),
        )
        .await??;

        if !response.status().is_success() {
            return Err(InstanceTypeError::ReceiveImds);
        }
        let bytes = response.bytes().await?;

        let contents = str::from_utf8(&bytes)?;

        Ok(Self(contents.to_string()))
    }

    // Uses instance metadata to determine if instance is bare metal.
    pub fn is_metal(&self) -> bool {
        self.0.contains("metal")
    }
}

#[cfg(test)]
mod test {
    use super::*;

    // This tests does NOT reach out to IMDS. Instead it uses `get_from_imds` specific to the test
    // binary which returns a hard coded value, `m7i.xlarge`.
    #[tokio::test]
    async fn from_imds() {
        let instance_type = InstanceType::get_from_imds().await.unwrap();
        println!("{:?}", instance_type);
    }

    #[test]
    fn metal_instance() {
        let instance_type = InstanceType("m7i.metal-24xlarge ".to_string());

        assert!(instance_type.is_metal())
    }

    #[test]
    fn nonmetal_instance() {
        let instance_type = InstanceType("r5a.large".to_string());

        assert!(!instance_type.is_metal())
    }
}
