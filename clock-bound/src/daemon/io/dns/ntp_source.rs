//! DNS NTP IO Source
//!
//! Handles NTP sampling from DNS-resolved pool addresses. Unlike [`IpAddrSource`](crate::daemon::io::IpAddrSource),
//! this source tracks consecutive timeouts and notifies the resolver when a target
//! becomes unreachable.

use std::{net::SocketAddr, sync::Arc};
use tokio::{
    sync::{mpsc, watch},
    time::Duration,
};
use tracing::{debug, error, info};

use super::resolver;
use crate::daemon::io::ntp::{DaemonInfo, ExtensionField, Ntp, NtpIoError};
use crate::daemon::io::{ClockDisruptionEvent, ControlRequest};
use crate::daemon::{async_ring_buffer, event, selected_clock::SelectedClockSource};

/// The amount of time between source polls in normal mode.
const INTERVAL: Duration = Duration::from_secs(16);
/// Per-sample timeout duration.
const SAMPLE_TIMEOUT: Duration = Duration::from_millis(100);

/// Maximum number of consecutive unreachable samples before the source is considered unreachable.
const MAX_CONSECUTIVE_UNREACHABLE: u32 = 5;

/// Contains data used to run the DNS pool NTP source runner.
///
/// This source handles NTP addresses that were resolved via DNS. It composes the base
/// [`Ntp`] struct for all common IO plumbing and adds timeout tracking to detect
/// unreachable resolvers.
///
/// # Invariants
///
/// `sent_unreachable_msg` is only ever set to `true` once. When it does so, it notifies
/// the resolver task to destruct the NTP task and replace with a new one. Because of this
/// once this task designates the address as unreachable, the application WILL tear it down.
///
/// If the host becomes reachable between that time, it does not matter.
#[derive(Debug)]
pub struct NtpSource {
    pool_domain: String,
    ntp: Ntp,
    /// Count of consecutive samples that indicated the source was unreachable (timeouts
    /// and routing/address-family failures). If this reaches `MAX_CONSECUTIVE_UNREACHABLE`,
    /// the source is considered unreachable.
    consecutive_unreachable_count: u32,
    /// Communicate with resolver task
    resolver_tx: mpsc::Sender<resolver::Message>,
    /// Guard to prevent sending duplicate messages unreachable messages to the resolver
    sent_unreachable_msg: bool,
}

impl NtpSource {
    /// Constructs a new `NtpSource` with the given parameters.
    #[expect(clippy::too_many_arguments)]
    pub fn construct(
        pool_domain: String,
        address: SocketAddr,
        event_sender: async_ring_buffer::Sender<event::Ntp>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        selected_clock: Arc<SelectedClockSource>,
        daemon_info: DaemonInfo,
        resolver_tx: mpsc::Sender<resolver::Message>,
    ) -> Self {
        let extensions = vec![ExtensionField::Fec2V1(daemon_info)];
        let ntp = Ntp::builder()
            .event_sender(event_sender)
            .ctrl_receiver(ctrl_receiver)
            .clock_disruption_receiver(clock_disruption_receiver)
            .selected_clock(selected_clock)
            .extensions(extensions)
            .socket_address(address)
            .timeout(SAMPLE_TIMEOUT)
            .interval(INTERVAL)
            .build();
        NtpSource {
            pool_domain,
            ntp,
            resolver_tx,
            consecutive_unreachable_count: 0,
            sent_unreachable_msg: false,
        }
    }

    /// `NtpSource` task runner.
    ///
    /// Samples NTP packets from the DNS-resolved address. Tracks consecutive timeouts
    /// and notifies the resolver if the source becomes unreachable.
    ///
    /// # Panics
    /// Function will panic if not called within the `tokio` runtime.
    ///
    /// # Errors
    /// Returns error if loop exits unexpectedly
    #[tracing::instrument(level = "info", skip_all, fields(pool = self.pool_domain, identifier = %self.ntp.socket_address()))]
    pub async fn run(&mut self) -> Result<(), NtpIoError> {
        debug!("Starting DNS NTP Source IO sampling loop.");
        self.register_with_resolver().await;
        loop {
            let branches = self.ntp.select_branches();
            tokio::select! {
                biased; // priority order is disruption, commands, and ticks
                val = branches.clock_disruption_receiver.changed() => {
                    if let Err(e) = val {
                        error!(?e, "Clock disruption receiver dropped.");
                        break;
                    }
                    info!("Received clock disruption signal.");
                    self.ntp.handle_disruption();
                }
                ctrl_req = branches.ctrl_receiver.recv() => {
                    match ctrl_req {
                        None => {
                            // this select can happen if `SourceIO` drops the ctrl_sender
                            break;
                        },
                        Some(ControlRequest::Shutdown) => {
                            debug!("Received shutdown signal. Exiting.");
                            break;
                        },
                    }
                }
                _ = branches.interval.tick() => {
                    self.handle_interval_tick().await;
                }
            }
        }
        debug!("DNS NTP Source IO runner exiting.");
        self.notify_resolver_shutdown().await;
        Ok(())
    }

    async fn handle_interval_tick(&mut self) {
        let sample_result = self.ntp.sample().await;
        self.handle_ntp_sample_result(sample_result).await;
    }

    async fn handle_ntp_sample_result(&mut self, result: Result<event::Ntp, NtpIoError>) {
        match result {
            // Errors that indicate the remote source is unreachable count toward the
            // consecutive failure budget and eventually retire the source. This covers
            // plain timeouts as well as bind/send failures caused by routing or
            // address-family problems (e.g. an IPv6 route or the IPv6 stack disappearing
            // underneath us). Gating reselection on these keeps the pool resilient when
            // the host's network stack changes, rather than pinning a dead source.
            Err(e) if e.indicates_unreachable_source() => {
                debug!(?e, "DNS NTP source unreachable.");
                self.consecutive_unreachable_count += 1;
                if !self.sent_unreachable_msg
                    && self.consecutive_unreachable_count >= MAX_CONSECUTIVE_UNREACHABLE
                {
                    self.sent_unreachable_msg = true;
                    self.notify_resolver_unreachable().await;
                }
            }
            // Remaining errors are transient local IO/hardware failures with no bearing on
            // the reachability of the remote address, so they are retried on the next tick.
            Err(e) => {
                debug!(?e, "Failed to sample DNS NTP source.");
            }
            Ok(ntp_event) => {
                self.consecutive_unreachable_count = 0;
                if self.ntp.send_event(&ntp_event).is_err() {
                    debug!("Attempted to send ntp event to buffer when it's shutting down");
                }
            }
        }
    }

    /// Registers this source with the DNS resolver.
    ///
    /// Called once at the beginning of the run loop to inform the resolver that
    /// this source is active and will be sampling.
    async fn register_with_resolver(&self) {
        let addr = self.ntp.socket_address().ip();
        if self
            .resolver_tx
            .send(resolver::Message::new_register_addr(addr))
            .await
            .is_err()
        {
            // Resolver has exited (e.g. during shutdown). This source will exit
            // on the next select iteration when it observes its own shutdown signal.
            info!("Resolver channel closed during registration, source will exit.");
        }
    }

    /// Notifies the resolver that this source is unreachable.
    ///
    /// Called when the source has been unreachable [`MAX_CONSECUTIVE_UNREACHABLE`] times
    /// in a row, indicating the resolved address may no longer be valid.
    async fn notify_resolver_unreachable(&self) {
        let addr = self.ntp.socket_address().ip();
        if self
            .resolver_tx
            .send(resolver::Message::new_unreachable_addr(addr))
            .await
            .is_err()
        {
            // Resolver has exited (e.g. during shutdown). This is expected when
            // a shutdown command races with timeout detection.
            info!("Resolver channel closed during unreachable notification, source will exit.");
        }
    }

    /// Notify the resolver that this source is shutting down
    ///
    /// Called when exiting the `run` loop
    async fn notify_resolver_shutdown(&mut self) {
        let addr = self.ntp.socket_address().ip();
        if self
            .resolver_tx
            .send(resolver::Message::new_ntp_task_shutdown(addr))
            .await
            .is_err()
        {
            // Resolver has exited (e.g. during shutdown). This is expected when
            // a shutdown command races with timeout detection.
            debug!("Resolver channel closed during unregister notification, source will exit.");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::event::{Ntp as NtpEvent, NtpData, Stratum};
    use crate::daemon::io::ClockDisruptionEvent;
    use crate::daemon::io::ntp::DaemonInfo;
    use crate::daemon::selected_clock::SelectedClockSource;
    use crate::daemon::time::{Duration as CbDuration, Instant as CbInstant, TscCount};
    use std::net::{Ipv4Addr, SocketAddrV4};

    /// Helper to create an `NtpSource` for unit testing.
    ///
    /// Returns the source along with the ring buffer receiver (which must stay alive
    /// for `send_event` to succeed) and the resolver message receiver (which must stay
    /// alive for the resolver notifications to succeed).
    fn create_test_ntp_source() -> (
        NtpSource,
        async_ring_buffer::Receiver<event::Ntp>,
        mpsc::Receiver<resolver::Message>,
    ) {
        let (event_sender, event_receiver) = async_ring_buffer::create::<event::Ntp>(4);
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (_clock_disruption_sender, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent {
                disruption_marker: None,
            });
        let (resolver_tx, resolver_rx) = mpsc::channel::<resolver::Message>(8);

        let selected_clock = Arc::new(SelectedClockSource::default());
        let daemon_info = DaemonInfo {
            major_version: 3,
            minor_version: 0,
            startup_id: 42,
        };
        let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 123));

        let source = NtpSource::construct(
            "pool.ntp.org".to_string(),
            address,
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            selected_clock,
            daemon_info,
            resolver_tx,
        );
        (source, event_receiver, resolver_rx)
    }

    /// Helper to create a valid `event::Ntp` value.
    fn create_ntp_event() -> NtpEvent {
        NtpEvent::builder()
            .counter_pre(TscCount::new(1000))
            .counter_post(TscCount::new(2000))
            .ntp_data(NtpData {
                server_recv_time: CbInstant::from_secs(10),
                server_send_time: CbInstant::from_secs(11),
                root_delay: CbDuration::from_micros(100),
                root_dispersion: CbDuration::from_micros(50),
                stratum: Stratum::ONE,
            })
            .build()
            .expect("valid NTP event")
    }

    /// Helper to create an `NtpIoError::Timeout` variant.
    ///
    /// `tokio::time::error::Elapsed` has a private constructor, so we obtain one
    /// by actually timing out a zero-duration future.
    async fn create_timeout_error() -> NtpIoError {
        let elapsed =
            tokio::time::timeout(Duration::from_nanos(0), futures::future::pending::<()>())
                .await
                .unwrap_err();
        NtpIoError::Timeout(elapsed)
    }

    #[tokio::test]
    async fn ok_result_sends_event_and_resets_consecutive_unreachable_count() {
        let (mut source, rx, _resolver_rx) = create_test_ntp_source();

        // Simulate some prior unreachable samples
        source.consecutive_unreachable_count = 3;

        let ntp_event = create_ntp_event();
        source.handle_ntp_sample_result(Ok(ntp_event.clone())).await;

        // Consecutive unreachable count should be reset to 0
        assert_eq!(source.consecutive_unreachable_count, 0);

        let sent_event = rx.recv().await.unwrap();
        assert_eq!(sent_event, ntp_event);
    }

    #[tokio::test]
    async fn ok_result_does_not_change_marked_unreachable() {
        let (mut source, _rx, _resolver_rx) = create_test_ntp_source();

        // Set marked_unreachable to true (simulating it was already triggered)
        source.sent_unreachable_msg = true;
        source.consecutive_unreachable_count = MAX_CONSECUTIVE_UNREACHABLE;

        let ntp_event = create_ntp_event();
        source.handle_ntp_sample_result(Ok(ntp_event)).await;

        // marked_unreachable should remain true even after a successful sample
        assert!(source.sent_unreachable_msg);
    }

    #[tokio::test]
    async fn timeout_error_increments_consecutive_unreachable_count() {
        let (mut source, _rx, _resolver_rx) = create_test_ntp_source();
        assert_eq!(source.consecutive_unreachable_count, 0);

        for expected_count in 1..MAX_CONSECUTIVE_UNREACHABLE {
            let err = create_timeout_error().await;
            source.handle_ntp_sample_result(Err(err)).await;
            assert_eq!(source.consecutive_unreachable_count, expected_count);
        }
    }

    #[tokio::test]
    async fn transient_error_does_not_increment_consecutive_unreachable_count() {
        let (mut source, _rx, _resolver_rx) = create_test_ntp_source();
        assert_eq!(source.consecutive_unreachable_count, 0);

        let io_err = NtpIoError::SampleIo(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        ));
        source.handle_ntp_sample_result(Err(io_err)).await;

        assert_eq!(source.consecutive_unreachable_count, 0);
        assert!(!source.sent_unreachable_msg);
    }

    #[tokio::test]
    async fn notifies_resolver_unreachable_when_consecutive_unreachable_reach_max() {
        let (mut source, _rx, mut resolver_rx) = create_test_ntp_source();

        // Feed timeout errors up to the maximum. The final one triggers
        // `notify_resolver_unreachable`, which sends an `UnreachableAddr` message.
        for _ in 0..MAX_CONSECUTIVE_UNREACHABLE {
            let err = create_timeout_error().await;
            source.handle_ntp_sample_result(Err(err)).await;
        }

        assert_eq!(
            source.consecutive_unreachable_count,
            MAX_CONSECUTIVE_UNREACHABLE
        );
        assert!(source.sent_unreachable_msg);

        let msg = resolver_rx.recv().await.expect("resolver message sent");
        let expected_addr = source.ntp.socket_address().ip();
        assert_eq!(msg, resolver::Message::new_unreachable_addr(expected_addr));
    }
}
