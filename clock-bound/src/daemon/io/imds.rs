//! Struct holding instance type meta data
use bytes::Bytes;
use core::str;
use reqwest::{Client, Response};
use std::time::Duration;
use thiserror::Error;
use tokio::time::timeout;
use tokio_retry::{
    Retry,
    strategy::{ExponentialBackoff, jitter},
};
use tracing::warn;

#[derive(Debug, Error)]
pub enum IMDSError {
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

    /// Request and parse info using the [IMDSv2 API]
    ///
    /// [IMDSv2 API]: https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configuring-instance-metadata-service.html
    pub async fn get_from_imds() -> Result<Self, IMDSError> {
        const MAX_REQUESTS: usize = 4;
        let client = reqwest::Client::new();

        let retry_strategy = ExponentialBackoff::from_millis(10)
            .map(jitter)
            .take(MAX_REQUESTS);

        // get session token
        let token_response = Retry::spawn(retry_strategy.clone(), || {
            Self::execute_imds_token_request(&client)
        })
        .await?;

        let token = token_response.bytes().await?;

        // // get actual metadata
        let instance_type_response = Retry::spawn(retry_strategy.clone(), || {
            Self::execute_imds_instance_type_request(&client, token.clone())
        })
        .await?;

        let instance_type_bytes = instance_type_response.bytes().await?;
        let instance_type = str::from_utf8(&instance_type_bytes)?;

        Ok(Self(instance_type.to_string()))
    }

    // Uses instance metadata to determine if instance is bare metal.
    pub fn is_metal(&self) -> bool {
        self.0.contains("metal")
    }

    /// Executes an IMDS request for an authentication token and returns the result
    async fn execute_imds_token_request(client: &Client) -> Result<Response, IMDSError> {
        let response_result = match timeout(
            Self::IMDS_TIMEOUT_DURATION,
            client
                .put(format!("{}/latest/api/token", Self::IMDS_ORIGIN))
                .header("X-aws-ec2-metadata-token-ttl-seconds", "21600")
                .send(),
        )
        .await
        {
            Ok(result) => result,
            Err(e) => {
                warn!("Timeout for IMDS request breached. Retrying.");
                Err(IMDSError::Timeout(e))?
            }
        };

        match response_result {
            Ok(response) => Ok(response),
            Err(e) => {
                warn!("Error returned by IMDS request: {}. Retrying.", e);
                Err(IMDSError::ReceiveToken)
            }
        }
    }

    /// Executes an IMDS request for instance type metadata and returns the result
    async fn execute_imds_instance_type_request(
        client: &Client,
        token: Bytes,
    ) -> Result<Response, IMDSError> {
        let response_result = match timeout(
            Self::IMDS_TIMEOUT_DURATION,
            client
                .get(format!(
                    "{}/latest/meta-data/instance-type",
                    Self::IMDS_ORIGIN
                ))
                .header("X-aws-ec2-metadata-token", &*token)
                .send(),
        )
        .await
        {
            Ok(result) => result,
            Err(e) => {
                warn!("Timeout for IMDS request breached. Retrying.");
                Err(IMDSError::Timeout(e))?
            }
        };

        match response_result {
            Ok(response) => Ok(response),
            Err(e) => {
                warn!("Error returned by IMDS request: {}. Retrying.", e);
                Err(IMDSError::ReceiveToken)
            }
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

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
