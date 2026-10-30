//! Partition resolver: picks the public Amazon Time Sync domain on Amazon EC2 instances
//! (Nitro and Xen).
//!
//! The public Amazon Time Sync domain depends on the AWS partition: `time.amazonaws.cn`
//! in `aws-cn`, `time.aws.com` everywhere else. The partition is read from `IMDSv2`
//! (`/latest/meta-data/services/partition`).
//!
//! [`PartitionResolver`] is a one-shot task:
//!
//! 1. Query IMDS until the partition is known. Failed attempts back off geometrically from
//!    1 s up to 60 s between retries.
//! 2. If no partition is known after [`PARTITION_DETECTION_TIMEOUT`] (10 minutes), give up
//!    and fall back to `time.aws.com`. This is the path taken by instances with the
//!    instance metadata option `http-endpoint disabled`, or where IMDS is unreachable.
//! 3. Send exactly one [`Dns::AddPool`](DnsMessage::AddPool) to the daemon and exit.
//!
//! On shutdown (or if the control channel closes) the task exits without sending anything.

mod imds;

use std::time::Duration;

#[cfg_attr(test, mockall_double::double)]
use imds::Imds;

use tokio::sync::mpsc;
use tokio::time::Instant;
use tracing::{debug, info, warn};

use crate::daemon::io::ControlRequest;
use crate::daemon::message::{AddPool, Dns as DnsMessage};

/// Delay before the first detection retry.
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);
/// Upper bound on the delay between detection retries.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
/// Give up on IMDS partition detection after this long and fall back to `time.aws.com`.
const PARTITION_DETECTION_TIMEOUT: Duration = Duration::from_mins(10);

/// One-shot task: detect the AWS partition, then ask the daemon to add the matching
/// public Amazon Time Sync pool.
#[derive(Debug)]
pub(crate) struct PartitionResolver {
    /// Daemon channel; receives exactly one `Dns::AddPool` on success or fallback.
    daemon_tx: mpsc::Sender<DnsMessage>,
    /// Receiver from the IO front end, observed for prompt shutdown.
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    /// `IMDSv2` client used to detect the partition.
    imds: Imds,
}

impl PartitionResolver {
    /// Constructs the partition resolver. No IO is performed.
    pub(crate) fn construct(
        daemon_tx: mpsc::Sender<DnsMessage>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
    ) -> Self {
        Self::construct_with_imds(Imds::new(), daemon_tx, ctrl_receiver)
    }

    /// Like [`PartitionResolver::construct`], with an explicit IMDS client.
    fn construct_with_imds(
        imds: Imds,
        daemon_tx: mpsc::Sender<DnsMessage>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
    ) -> Self {
        Self {
            daemon_tx,
            ctrl_receiver,
            imds,
        }
    }

    /// Detect the partition and send one `Dns::AddPool` for the matching domain.
    ///
    /// Falls back to `time.aws.com` after [`PARTITION_DETECTION_TIMEOUT`]. Returns without
    /// sending if shutdown is requested (or the control channel closes).
    #[tracing::instrument(level = "info", skip_all)]
    pub(crate) async fn run(&mut self) {
        let deadline = Instant::now() + PARTITION_DETECTION_TIMEOUT;
        // All cancellation is handled here: `detect` itself never observes the control
        // channel, and is dropped (mid-request or mid-backoff) when another arm wins.
        let pool_domain = tokio::select! {
            biased;
            ctrl = self.ctrl_receiver.recv() => {
                log_ctrl_exit(ctrl.as_ref());
                return;
            }
            () = tokio::time::sleep_until(deadline) => {
                let domain = Partition::Aws.domain();
                warn!(
                    timeout_secs = PARTITION_DETECTION_TIMEOUT.as_secs(),
                    domain,
                    "AWS partition detection via IMDSv2 timed out. IMDS may be disabled \
                     (instance metadata option `http-endpoint disabled`) or unreachable \
                     (e.g. hop limit or firewall). Falling back to the default domain."
                );
                domain
            }
            partition = Self::detect(&mut self.imds) => {
                info!(
                    ?partition,
                    domain = partition.domain(),
                    "Detected AWS partition."
                );
                partition.domain()
            }
        };
        self.send_message_add_pool(pool_domain).await;
    }

    /// Query IMDS until the partition is known, backing off between failed attempts.
    ///
    /// Never gives up on its own: the caller bounds it with a deadline and cancels it on
    /// shutdown. Cancel safe.
    async fn detect(imds: &mut Imds) -> Partition {
        let mut failed_attempts: u32 = 0;

        let mut next_attempt = Instant::now();
        loop {
            let error = match imds.fetch_partition().await {
                Ok(partition) => return partition,
                Err(error) => error,
            };
            let retry_in = backoff_delay(failed_attempts);
            next_attempt += retry_in;
            failed_attempts = failed_attempts.saturating_add(1);
            debug!(
                ?error,
                retry_in_secs = retry_in.as_secs(),
                "AWS partition detection via IMDSv2 failed. Retrying."
            );
            tokio::time::sleep_until(next_attempt).await;
        }
    }

    /// Send `Dns::AddPool` for `pool_domain` to the daemon.
    ///
    /// Races the send against the control channel so a full daemon channel cannot block
    /// shutdown.
    async fn send_message_add_pool(&mut self, pool_domain: &str) {
        let msg = DnsMessage::AddPool(AddPool {
            pool_domain: pool_domain.to_owned(),
        });
        tokio::select! {
            biased;
            ctrl = self.ctrl_receiver.recv() => log_ctrl_exit(ctrl.as_ref()),
            result = self.daemon_tx.send(msg) => match result {
                Ok(()) => info!(pool_domain, "Requested public Amazon Time Sync pool."),
                Err(error) => {
                    let message = "Daemon channel closed before AddPool could be sent.";
                    warn!(
                        ?error,
                        pool_domain,
                        %message
                    );
                    panic!("{message}");
                }
            },
        }
    }
}

/// AWS partition, as far as the public Amazon Time Sync domain is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Partition {
    /// Every partition other than `aws-cn`.
    Aws,
    /// AWS China (`aws-cn`).
    AwsCn,
}

impl Partition {
    /// `aws-cn` maps to [`Partition::AwsCn`]; any other value maps to [`Partition::Aws`].
    fn from_imds(raw: &str) -> Self {
        if raw == "aws-cn" {
            Self::AwsCn
        } else {
            Self::Aws
        }
    }

    /// Public Amazon Time Sync domain for this partition.
    fn domain(self) -> &'static str {
        match self {
            Self::Aws => "time.aws.com",
            Self::AwsCn => "time.amazonaws.cn",
        }
    }
}

/// Delay before the next attempt after `failed_attempts` prior failures.
///
/// 1 s, 2 s, 4 s, ... capped at 60 s.
fn backoff_delay(failed_attempts: u32) -> Duration {
    INITIAL_RETRY_DELAY
        .saturating_mul(2_u32.saturating_pow(failed_attempts))
        .min(MAX_RETRY_DELAY)
}

/// Log why the partition resolver is exiting on a control channel event.
fn log_ctrl_exit(ctrl: Option<&ControlRequest>) {
    match ctrl {
        Some(ControlRequest::Shutdown) => {
            debug!("Received shutdown in partition resolver. Exiting.");
        }
        None => debug!("Control channel closed in partition resolver. Exiting."),
    }
}

#[cfg(test)]
mod tests {
    use mockall::Sequence;
    use tokio::sync::mpsc::error::TryRecvError;

    use super::*;

    /// Build a resolver around `imds`, returning the control sender and daemon receiver
    /// so the channels stay open. The daemon channel has capacity 1.
    fn make(
        imds: Imds,
    ) -> (
        PartitionResolver,
        mpsc::Sender<ControlRequest>,
        mpsc::Receiver<DnsMessage>,
    ) {
        let (daemon_tx, daemon_rx) = mpsc::channel(1);
        let (ctrl_tx, ctrl_receiver) = mpsc::channel(1);
        let resolver = PartitionResolver::construct_with_imds(imds, daemon_tx, ctrl_receiver);
        (resolver, ctrl_tx, daemon_rx)
    }

    /// A mock IMDS that returns `partition` on the first and only call.
    fn imds_returning(partition: Partition) -> Imds {
        let mut imds = Imds::default();
        imds.expect_fetch_partition()
            .times(1)
            .returning(move || Ok(partition));
        imds
    }

    /// A mock IMDS that fails on every call.
    fn imds_failing() -> Imds {
        let mut imds = Imds::default();
        imds.expect_fetch_partition()
            .returning(|| Err(imds::Error::Timeout));
        imds
    }

    fn add_pool(pool_domain: &str) -> DnsMessage {
        DnsMessage::AddPool(AddPool {
            pool_domain: pool_domain.to_owned(),
        })
    }

    #[test]
    fn backoff_delay_sequence() {
        let delays: Vec<u64> = (0..=8).map(|n| backoff_delay(n).as_secs()).collect();
        assert_eq!(delays, vec![1, 2, 4, 8, 16, 32, 60, 60, 60]);
        assert_eq!(backoff_delay(u32::MAX), MAX_RETRY_DELAY);
    }

    #[test]
    fn partition_from_imds_and_domain() {
        for (raw, partition, domain) in [
            ("aws", Partition::Aws, "time.aws.com"),
            ("aws-cn", Partition::AwsCn, "time.amazonaws.cn"),
            ("aws-us-gov", Partition::Aws, "time.aws.com"),
            ("aws-iso", Partition::Aws, "time.aws.com"),
            ("aws-iso-b", Partition::Aws, "time.aws.com"),
            ("garbage", Partition::Aws, "time.aws.com"),
            ("", Partition::Aws, "time.aws.com"),
        ] {
            let parsed = Partition::from_imds(raw);
            assert_eq!(parsed, partition, "raw partition {raw:?}");
            assert_eq!(parsed.domain(), domain, "raw partition {raw:?}");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn detect_aws_cn() {
        let mut imds = imds_returning(Partition::AwsCn);
        assert_eq!(PartitionResolver::detect(&mut imds).await, Partition::AwsCn);
    }

    #[tokio::test(start_paused = true)]
    async fn detect_aws() {
        let mut imds = imds_returning(Partition::Aws);
        assert_eq!(PartitionResolver::detect(&mut imds).await, Partition::Aws);
    }

    #[tokio::test(start_paused = true)]
    async fn detect_retries_with_backoff_until_success() {
        // Two failures (1 s then 2 s backoff), then success on the third attempt.
        let mut imds = Imds::default();
        let mut seq = Sequence::new();
        imds.expect_fetch_partition()
            .times(2)
            .in_sequence(&mut seq)
            .returning(|| Err(imds::Error::Status(503)));
        imds.expect_fetch_partition()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|| Ok(Partition::AwsCn));

        let start = Instant::now();
        assert_eq!(PartitionResolver::detect(&mut imds).await, Partition::AwsCn);
        let elapsed = start.elapsed();
        assert!(
            (Duration::from_secs(3)..Duration::from_secs(4)).contains(&elapsed),
            "expected 1 s + 2 s of backoff, got {elapsed:?}"
        );
    }

    /// Shutdown cancels `detect` while it is waiting out a backoff.
    #[tokio::test(start_paused = true)]
    async fn run_exits_on_shutdown_during_backoff() {
        let mut imds = Imds::default();
        imds.expect_fetch_partition()
            .times(1)
            .returning(|| Err(imds::Error::Timeout));
        let (mut resolver, ctrl_tx, mut daemon_rx) = make(imds);

        let start = Instant::now();
        tokio::join!(resolver.run(), async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            ctrl_tx.send(ControlRequest::Shutdown).await.unwrap();
        });
        assert!(
            start.elapsed() < INITIAL_RETRY_DELAY,
            "run waited for the backoff to end"
        );
        assert_eq!(daemon_rx.try_recv(), Err(TryRecvError::Empty));
    }

    /// Covers the ctrl arm winning before `detect` is ever polled: mockall can't hold a
    /// fetch in flight, so queue the shutdown first and rely on the biased select.
    #[tokio::test(start_paused = true)]
    async fn run_exits_on_shutdown_queued_before_fetch() {
        let mut imds = Imds::default();
        imds.expect_fetch_partition().never();
        let (mut resolver, ctrl_tx, mut daemon_rx) = make(imds);
        ctrl_tx.send(ControlRequest::Shutdown).await.unwrap();
        resolver.run().await;
        assert_eq!(daemon_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[tokio::test(start_paused = true)]
    async fn run_sends_add_pool_for_aws_cn() {
        let (mut resolver, _ctrl_tx, mut daemon_rx) = make(imds_returning(Partition::AwsCn));
        resolver.run().await;
        assert_eq!(daemon_rx.try_recv(), Ok(add_pool("time.amazonaws.cn")));
        assert_eq!(daemon_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[tokio::test(start_paused = true)]
    async fn run_sends_add_pool_for_aws() {
        let (mut resolver, _ctrl_tx, mut daemon_rx) = make(imds_returning(Partition::Aws));
        resolver.run().await;
        assert_eq!(daemon_rx.try_recv(), Ok(add_pool("time.aws.com")));
        assert_eq!(daemon_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[tokio::test(start_paused = true)]
    async fn run_falls_back_to_time_aws_com_after_timeout() {
        let (mut resolver, _ctrl_tx, mut daemon_rx) = make(imds_failing());
        let start = Instant::now();
        resolver.run().await;
        let elapsed = start.elapsed();
        assert!(
            (PARTITION_DETECTION_TIMEOUT..PARTITION_DETECTION_TIMEOUT + Duration::from_secs(1))
                .contains(&elapsed),
            "expected fallback at the detection timeout, got {elapsed:?}"
        );
        assert_eq!(daemon_rx.try_recv(), Ok(add_pool("time.aws.com")));
        assert_eq!(daemon_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[tokio::test(start_paused = true)]
    async fn run_exits_on_shutdown_without_sending() {
        let (mut resolver, ctrl_tx, mut daemon_rx) = make(imds_failing());
        let start = Instant::now();
        tokio::join!(resolver.run(), async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            ctrl_tx.send(ControlRequest::Shutdown).await.unwrap();
        });
        assert!(start.elapsed() < PARTITION_DETECTION_TIMEOUT);
        assert_eq!(daemon_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[tokio::test(start_paused = true)]
    async fn run_exits_when_ctrl_closed_without_sending() {
        let mut imds = Imds::default();
        imds.expect_fetch_partition().never();
        let (mut resolver, ctrl_tx, mut daemon_rx) = make(imds);
        drop(ctrl_tx);
        resolver.run().await;
        assert_eq!(daemon_rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[tokio::test(start_paused = true)]
    async fn send_message_add_pool_exits_on_shutdown_when_daemon_channel_full() {
        let (mut resolver, ctrl_tx, mut daemon_rx) = make(Imds::default());
        resolver.daemon_tx.try_send(add_pool("filler")).unwrap();
        ctrl_tx.send(ControlRequest::Shutdown).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            resolver.send_message_add_pool("time.aws.com"),
        )
        .await
        .expect("send_message_add_pool blocked on a full daemon channel during shutdown");
        assert_eq!(daemon_rx.try_recv(), Ok(add_pool("filler")));
        assert_eq!(daemon_rx.try_recv(), Err(TryRecvError::Empty));
    }
}
