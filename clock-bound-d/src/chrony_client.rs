//! Abstractions to connect to a chrony client

use std::time::{Duration, Instant};

use anyhow::Context;
use chrony_candm::{
    blocking_query_uds,
    common::ChronyAddr,
    reply::{Reply, ReplyBody, Status, Tracking},
    request::{Burst, RequestBody},
    ClientOptions,
};
use retry::{delay::Fixed, retry_with_index};
use tracing::info;

/// The default client options for Chrony communications in ClockBound.
///
/// The number of tries is set to 1 because retries are performed in the
/// ClockBound code so that we can have logs about the retry
/// attempts.
const CHRONY_CANDM_CLIENT_OPTIONS: ClientOptions = ClientOptions {
    timeout: Duration::from_secs(1),
    n_tries: 1,
};

/// Convenience trait for requesting information from Chrony
///
/// The only fn that needs to be implemented is [`ChronyClient::query`]. After that, the default
/// implementations of the trait will be able to write to request the various metrics
#[cfg_attr(any(test, feature = "test"), mockall::automock)]
pub trait ChronyClient: Send {
    /// Polls `chrony` for user requested statistics.
    fn query(&self, request_body: RequestBody) -> std::io::Result<Reply>;
}

#[cfg(any(test, feature = "test"))]
impl ChronyClientExt for MockChronyClient {}

impl core::fmt::Debug for (dyn ChronyClient + '_) {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("dyn ChronyClient")
    }
}

/// Extension trait on [`ChronyClient`] to implement high level chrony commands, like getting `tracking`.
pub trait ChronyClientExt: ChronyClient {
    /// Polls `chrony` for tracking info
    fn query_tracking(&self) -> anyhow::Result<Tracking> {
        // Queries tracking data using `chronyc tracking`.
        let request_body = RequestBody::Tracking;
        let reply = self.query(request_body).context("query tracking")?;

        // Verifying query contains expected, tracking, metrics.
        let ReplyBody::Tracking(tracking) = reply.body else {
            anyhow::bail!(
                "Reply body does not contain tracking statistics. {:?}",
                reply
            );
        };

        Ok(tracking)
    }

    /// Send command to chronyd to reset its sources
    ///
    /// Note that this is only supported by chronyd >= 4.0
    /// TODO: if chronyd is running a version < 4.0, may have to delete and add the peer back instead.
    fn reset_sources(&self) -> anyhow::Result<()> {
        let request_body = RequestBody::ResetSources;
        let reply = self.query(request_body).context("reset chronyd")?;
        if reply.status == Status::Success {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Bad reply status {:?}", reply.status))
        }
    }

    /// Send command to chronyd to send burst requests to its sources.
    ///
    /// Note that this is supported by chronyd >= 2.4
    fn burst_sources(&self) -> anyhow::Result<()> {
        let burst_params = Burst {
            mask: ChronyAddr::Unspec,
            address: ChronyAddr::Unspec,
            n_good_samples: 4,
            n_total_samples: 8,
        };
        let request_body = RequestBody::Burst(burst_params);
        let reply = self.query(request_body).context("burst chronyd")?;
        if reply.status == Status::Success {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Bad reply status {:?}", reply.status))
        }
    }

    /// Helper function, to reset chronyd and quickly poll for new samples.
    ///
    /// When we recover from a clock disruption, we want to make sure Chronyd gets reset and try to help it recover quickly.
    /// Thus, we try to reset chronyd and then burst our upstream time sources for more samples.
    fn reset_chronyd_with_retries(&self, num_retries: usize) -> anyhow::Result<()> {
        let num_attempts = num_retries + 1;
        retry_with_index(
            Fixed::from_millis(5).take(num_retries), |attempt_number| {
                let attempt_start_instant = Instant::now();
                self.reset_sources()
                    .inspect(|_| {
                        let attempt_duration = attempt_start_instant.elapsed();
                        info!(
                            attempt = %attempt_number,
                            "Resetting chronyd sources (attempt {:?} of {:?}) was successful.  Attempt duration: {:?}",
                            attempt_number,
                            num_attempts,
                            attempt_duration
                        );
                    })
                    .inspect_err(|e| {
                        let attempt_duration = attempt_start_instant.elapsed();
                        info!(
                            attempt = %attempt_number,
                            "Resetting chronyd sources (attempt {:?} of {:?}) was unsuccessful.  Err({:?}).  Attempt duration: {:?}",
                            attempt_number,
                            num_attempts,
                            e,
                            attempt_duration
                        );
                    })
            }
        ).map_err(|e| anyhow::anyhow!("Failed to reset chronyd after {:?} attempts.  Err({:?})", num_attempts, e))?;

        retry_with_index(
            Fixed::from_millis(100).take(num_retries), |attempt_number| {
                let attempt_start_instant = Instant::now();
                self.burst_sources()
                    .inspect(|_| {
                        let attempt_duration = attempt_start_instant.elapsed();
                        info!(
                            attempt = %attempt_number,
                            "Bursting chronyd sources (attempt {:?} of {:?}) was successful.  Attempt duration: {:?}",
                            attempt_number,
                            num_attempts,
                            attempt_duration
                        );
                    })
                    .inspect_err(|e| {
                        let attempt_duration = attempt_start_instant.elapsed();
                        info!(
                            attempt = %attempt_number,
                            "Bursting chronyd sources (attempt {:?} of {:?}) was unsuccessful.  Err({:?}).  Attempt duration: {:?}",
                            attempt_number,
                            num_attempts,
                            e,
                            attempt_duration
                        );
                    })
            }
        ).map_err(|e| anyhow::anyhow!("Failed to burst chronyd after {:?} attempts.  Err({:?})", num_attempts, e))
    }
}

#[cfg(any(test, feature = "test"))]
mod mock_chrony_client_ext {
    use super::*;
    mockall::mock! {
        pub ChronyClientExt {}

        impl ChronyClientExt for ChronyClientExt {
            fn query_tracking(&self) -> anyhow::Result<Tracking>;
            fn reset_sources(&self) -> anyhow::Result<()>;
            fn burst_sources(&self) -> anyhow::Result<()>;
            fn reset_chronyd_with_retries(&self, num_attempts: usize) -> anyhow::Result<()>;
        }
    }

    impl ChronyClient for MockChronyClientExt {
        fn query(&self, _request_body: RequestBody) -> std::io::Result<Reply> {
            unimplemented!("mocks shouldn't call this")
        }
    }
}

#[cfg(any(test, feature = "test"))]
pub use mock_chrony_client_ext::MockChronyClientExt;

impl core::fmt::Debug for (dyn ChronyClientExt + '_) {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("dyn ChronyClientExt")
    }
}

/// Unix Domain Socket client for Chrony-CandM protocol.
///
/// Getting Tracking data is a read-only operation. The chronyd daemon accepts these operations
/// over both a UDS as well as a UDP socket over the IPv4/IPv6 loopback addresses by default.
/// To support clock disruption, however, chronyd may be instructed to be reset. This mutable
/// operations are only accepted over a local UDS socket.
///
/// The use of a UDS socket brings all sorts of permission issues. In particular, if chronyd
/// runs as the "chrony" user, chronyd sets the permissions on the UDS to the "chrony" user
/// only. So ... we don't want to wait for a clock disruption event to realize we have a
/// permission problem. Hence, even if the UDS socket is not strictly required here, we use it
/// to have an early and periodic signal that things are off.
pub struct UnixDomainSocket {
    client_options: ClientOptions,
}

impl Default for UnixDomainSocket {
    fn default() -> Self {
        Self {
            client_options: CHRONY_CANDM_CLIENT_OPTIONS,
        }
    }
}

impl ChronyClient for UnixDomainSocket {
    fn query(&self, request_body: RequestBody) -> std::io::Result<Reply> {
        blocking_query_uds(request_body, self.client_options)
    }
}

impl ChronyClientExt for UnixDomainSocket {}

#[cfg(test)]
mod test {
    use super::*;
    use chrony_candm::common::{ChronyAddr, ChronyFloat};
    use rstest::rstest;

    fn internal_error_response() -> chrony_candm::reply::Reply {
        Reply {
            status: chrony_candm::reply::Status::Failed,
            cmd: 0,
            sequence: 0,
            body: ReplyBody::Null,
        }
    }

    fn example_candm_tracking() -> chrony_candm::reply::Reply {
        let tracking = chrony_candm::reply::Tracking {
            ref_id: 0u32,
            ip_addr: ChronyAddr::Unspec,
            stratum: 1u16,
            leap_status: 0u16,
            ref_time: std::time::SystemTime::now(),
            current_correction: ChronyFloat::default(),
            last_offset: ChronyFloat::default(),
            rms_offset: ChronyFloat::default(),
            freq_ppm: ChronyFloat::default(),
            resid_freq_ppm: ChronyFloat::default(),
            skew_ppm: ChronyFloat::default(),
            root_delay: ChronyFloat::default(),
            root_dispersion: ChronyFloat::default(),
            last_update_interval: ChronyFloat::default(),
        };

        Reply {
            status: chrony_candm::reply::Status::Success,
            cmd: 0,
            sequence: 0,
            body: ReplyBody::Tracking(tracking),
        }
    }

    fn example_success_reply() -> Reply {
        Reply {
            status: Status::Success,
            cmd: 0,
            sequence: 0,
            body: ReplyBody::Null,
        }
    }

    fn example_fail_reply() -> Reply {
        Reply {
            status: Status::Failed,
            cmd: 0,
            sequence: 0,
            body: ReplyBody::Null,
        }
    }

    /// Test verifying failure modes of `gather_metrics`. If the mock chrony client returns a
    /// `Err` or a success with an unexpected return type we expect an `Err` as a result.
    #[rstest]
    #[case::io_error(Err(std::io::Error::new(std::io::ErrorKind::Other, "oops")))]
    #[case::wrong_response(Ok(internal_error_response()))]
    fn test_chrony_tracking_fail(#[case] return_value: std::io::Result<Reply>) {
        let mut mock_chrony_client = MockChronyClient::new();

        mock_chrony_client
            .expect_query()
            .once()
            .withf(|body| matches!(body, RequestBody::Tracking))
            .return_once(|_| return_value);

        let rt = mock_chrony_client.query_tracking();
        assert!(rt.is_err());
    }

    #[test]
    fn test_chrony_tracking_success() {
        let mut mock_chrony_client = MockChronyClient::new();

        mock_chrony_client
            .expect_query()
            .once()
            .withf(|body| matches!(body, RequestBody::Tracking))
            .return_once(|_| Ok(example_candm_tracking()));
        let rt = mock_chrony_client.query_tracking();
        assert!(rt.is_ok());
    }

    #[rstest]
    #[case::io_error(Err(std::io::Error::new(std::io::ErrorKind::Other, "oops")))]
    #[case::wrong_response(Ok(internal_error_response()))]
    fn test_chrony_reset_sources_fail(#[case] return_value: std::io::Result<Reply>) {
        let mut mock_chrony_client = MockChronyClient::new();

        mock_chrony_client
            .expect_query()
            .once()
            .withf(|body| matches!(body, RequestBody::ResetSources))
            .return_once(|_| return_value);

        let rt = mock_chrony_client.reset_sources();
        assert!(rt.is_err());
    }

    #[test]
    fn test_chrony_reset_sources_success() {
        let mut mock_chrony_client = MockChronyClient::new();
        mock_chrony_client
            .expect_query()
            .once()
            .withf(|body| matches!(body, RequestBody::ResetSources))
            .return_once(|_| {
                Ok(Reply {
                    status: Status::Success,
                    cmd: 0,
                    sequence: 0,
                    body: ReplyBody::Null,
                })
            });
        let rt = mock_chrony_client.reset_sources();
        assert!(rt.is_ok());
    }

    #[rstest]
    #[case::io_error(Err(std::io::Error::new(std::io::ErrorKind::Other, "oops")))]
    #[case::wrong_response(Ok(internal_error_response()))]
    fn test_chrony_burst_sources_fail(#[case] return_value: std::io::Result<Reply>) {
        let mut mock_chrony_client = MockChronyClient::new();
        mock_chrony_client
            .expect_query()
            .once()
            .withf(|body| {
                matches!(
                    body,
                    RequestBody::Burst(Burst {
                        mask: ChronyAddr::Unspec,
                        address: ChronyAddr::Unspec,
                        n_good_samples: 4,
                        n_total_samples: 8,
                    })
                )
            })
            .return_once(|_| return_value);
        let rt = mock_chrony_client.burst_sources();
        assert!(rt.is_err());
    }

    #[test]
    fn test_chrony_burst_sources_success() {
        let mut mock_chrony_client = MockChronyClient::new();
        mock_chrony_client
            .expect_query()
            .once()
            .withf(|body| {
                matches!(
                    body,
                    RequestBody::Burst(Burst {
                        mask: ChronyAddr::Unspec,
                        address: ChronyAddr::Unspec,
                        n_good_samples: 4,
                        n_total_samples: 8,
                    })
                )
            })
            .return_once(|_| {
                Ok(Reply {
                    status: Status::Success,
                    cmd: 0,
                    sequence: 0,
                    body: ReplyBody::Null,
                })
            });
        let rt = mock_chrony_client.burst_sources();
        assert!(rt.is_ok());
    }

    #[rstest]
    #[case::succeed_on_first_try_for_both_requests(
        vec![example_success_reply()],
        vec![example_success_reply()],
        1,
        1,
        10
    )]
    #[case::succeed_after_some_fails_for_both_requests(
        vec![example_fail_reply(), example_fail_reply(), example_success_reply()],
        vec![example_fail_reply(), example_success_reply()],
        3,
        2,
        10
    )]
    #[case::single_attempt_with_no_retries_success(
        vec![example_success_reply()],
        vec![example_success_reply()],
        1,
        1,
        0
    )]
    fn test_reset_chronyd_with_retries_success(
        #[case] reset_return_values: Vec<Reply>,
        #[case] burst_return_values: Vec<Reply>,
        #[case] expected_reset_call_count: usize,
        #[case] expected_burst_call_count: usize,
        #[case] num_attempts: usize,
    ) {
        let mut sequence = mockall::Sequence::new();
        let mut mock_chrony_client = MockChronyClient::new();

        let mut reset_return_values = reset_return_values.into_iter();
        let mut burst_return_values = burst_return_values.into_iter();

        mock_chrony_client
            .expect_query()
            .times(expected_reset_call_count)
            .withf(|body| matches!(body, RequestBody::ResetSources))
            .returning(move |_| Ok(reset_return_values.next().unwrap()))
            .in_sequence(&mut sequence);
        mock_chrony_client
            .expect_query()
            .times(expected_burst_call_count)
            .withf(|body| {
                matches!(
                    body,
                    RequestBody::Burst(Burst {
                        mask: ChronyAddr::Unspec,
                        address: ChronyAddr::Unspec,
                        n_good_samples: 4,
                        n_total_samples: 8,
                    })
                )
            })
            .returning(move |_| Ok(burst_return_values.next().unwrap()))
            .in_sequence(&mut sequence);
        let res = mock_chrony_client.reset_chronyd_with_retries(num_attempts);
        assert!(res.is_ok());
    }

    #[rstest]
    #[case::fail_after_too_many_reset_sources_fails(
        vec![example_fail_reply(); 10],
        vec![],
        10,
        0,
        9
    )]
    #[case::fail_after_too_many_burst_sources_fails(
        vec![example_success_reply()],
        vec![example_fail_reply(); 10],
        1,
        10,
        9
    )]
    fn test_reset_chronyd_with_retries_failure(
        #[case] reset_return_values: Vec<Reply>,
        #[case] burst_return_values: Vec<Reply>,
        #[case] expected_reset_call_count: usize,
        #[case] expected_burst_call_count: usize,
        #[case] num_attempts: usize,
    ) {
        let mut sequence = mockall::Sequence::new();
        let mut mock_chrony_client = MockChronyClient::new();

        let mut reset_return_values = reset_return_values.into_iter();
        let mut burst_return_values = burst_return_values.into_iter();

        mock_chrony_client
            .expect_query()
            .times(expected_reset_call_count)
            .withf(|body| matches!(body, RequestBody::ResetSources))
            .returning(move |_| Ok(reset_return_values.next().unwrap()))
            .in_sequence(&mut sequence);
        mock_chrony_client
            .expect_query()
            .times(expected_burst_call_count)
            .withf(|body| {
                matches!(
                    body,
                    RequestBody::Burst(Burst {
                        mask: ChronyAddr::Unspec,
                        address: ChronyAddr::Unspec,
                        n_good_samples: 4,
                        n_total_samples: 8,
                    })
                )
            })
            .returning(move |_| Ok(burst_return_values.next().unwrap()))
            .in_sequence(&mut sequence);
        let res = mock_chrony_client.reset_chronyd_with_retries(num_attempts);
        assert!(res.is_err());
    }
}
