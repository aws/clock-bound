//! Amazon Time Sync IO Source

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info};

use super::ntp::{BurstConfig, Ntp, NtpIoError};
use super::{ClockDisruptionEvent, ControlRequest};
use crate::daemon::{async_ring_buffer, event, selected_clock::SelectedClockSource};

pub const BURST_DURATION: Duration = Duration::from_secs(1);
pub const BURST_INTERVAL: Duration = Duration::from_millis(50);

pub const ADDRESS_V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 123);
pub const ADDRESS_V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x123);
const PORT: u16 = 123;
const INTERVAL_DURATION: Duration = Duration::from_secs(2);
const SAMPLE_TIMEOUT: Duration = Duration::from_millis(100);

/// Maximum number of consecutive unreachable samples before swapping between network stacks
///
/// Value is not 1 to allow for spurious network changes
const MAX_CONSECUTIVE_FAILURE: u32 = 3;

/// Contains the data needed to run the Amazon Time Sync runner.
#[derive(Debug)]
pub struct AmazonTimeSync {
    ntp: Ntp,
    /// Count of consecutive failures seen for the current network stack
    ///
    /// If this reaches `MAX_CONSECUTIVE_FAILURE`, swap network stacks
    consecutive_failure_count: u32,
}

impl AmazonTimeSync {
    /// Constructs a new `AmazonTimeSync` with using given parameters.
    ///
    /// NOTE:
    /// The `AmazonTimeSync` object will start in burst mode. The timer for burst mode begins when
    /// the object is constructed, NOT when the run loop begins.
    pub fn construct(
        event_sender: async_ring_buffer::Sender<event::Ntp>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        selected_clock: Arc<SelectedClockSource>,
    ) -> Self {
        let burst_config = BurstConfig {
            duration: BURST_DURATION,
            interval: BURST_INTERVAL,
        };

        // Starting with IPv4
        let socket_addr = SocketAddr::new(IpAddr::from(ADDRESS_V4), PORT);

        let ntp = Ntp::builder()
            .event_sender(event_sender)
            .ctrl_receiver(ctrl_receiver)
            .clock_disruption_receiver(clock_disruption_receiver)
            .selected_clock(selected_clock)
            .extensions(vec![]) // No extensions for Amazon Time Sync
            .socket_address(socket_addr)
            .timeout(SAMPLE_TIMEOUT)
            .interval(INTERVAL_DURATION)
            .burst_config(burst_config)
            .build();

        info!(address = %ntp.socket_address().ip(), "Amazon Time Sync source initialized.");

        AmazonTimeSync {
            ntp,
            consecutive_failure_count: 0,
        }
    }

    /// NTP Amazon Time Sync task runner.
    ///
    /// Samples NTP packets from the AWS EC2 Amazon Time Sync address.
    ///
    /// The function runs in two modes a normal mode and a burst mode.
    ///
    /// While in burst mode the Amazon Time Sync source is polled more frequently
    /// Burst mode is triggered when:
    /// - a clock disruption signal is received.
    /// - ...
    ///
    /// Burst mode is active for a set amount of time, [`AMAZON_TIME_SYNC_BURST_DURATION`], before
    /// transitioning back to normal mode.
    ///
    /// # Panics
    /// Function will panic if not called within the `tokio` runtime.
    ///
    /// # Errors
    /// Returns error if loop exits unexpectedly
    #[tracing::instrument(
        level = "info",
        skip_all,
        fields(identifier = "local Amazon Time Sync NTP")
    )]
    pub async fn run(&mut self) -> Result<(), NtpIoError> {
        debug!("Starting Amazon Time Sync sampling loop.");
        loop {
            let branches = self.ntp.select_branches();
            tokio::select! {
                biased; // priority order is disruption, commands, and ticks
                val = branches.clock_disruption_receiver.changed() => {
                    if let Err(e) = val {
                        error!(?e, "Clock disruption receiver dropped.");
                        break;
                    }
                    if self.ntp.handle_disruption() {
                        info!("Amazon Time Sync source transitioning from `Normal` mode to `Burst` mode.");
                    }
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
        debug!("Amazon Time Sync runner exiting.");
        Ok(())
    }

    async fn handle_interval_tick(&mut self) {
        let ntp_result = self.ntp.sample().await;
        self.handle_ntp_sample_result(ntp_result);

        if self.ntp.handle_burst_expiry() {
            info!("Amazon Time Sync source transitioning from `Burst` mode to `Normal` mode.");
        }
    }

    // If sample is valid, send along to the ring buffer
    //
    // Otherwise, swap between network stacks if local network errors
    fn handle_ntp_sample_result(&mut self, result: Result<event::Ntp, NtpIoError>) {
        match result {
            Err(e) => {
                debug!(?e, "Failed to sample Amazon Time Sync NTP");
                // Only action based on local routing failures. This indicates something
                // changed on the network stack (example, dual stack -> IPv6 only)
                //
                // Ignore timeouts to prevent thrashing
                if e.indicates_local_routing_failure() {
                    self.consecutive_failure_count += 1;
                    if self.consecutive_failure_count >= MAX_CONSECUTIVE_FAILURE {
                        self.swap_network_stack();
                        self.consecutive_failure_count = 0;
                    }
                }
            }
            Ok(ntp_event) => {
                self.consecutive_failure_count = 0;
                if let Err(e) = self.ntp.send_event(&ntp_event) {
                    error!(?e, "channel closed. Not supported on this task");
                    panic!("unable to communicate with daemon. {e:?}");
                }
            }
        }
    }

    fn swap_network_stack(&mut self) {
        let current_addr = self.ntp.socket_address();
        let new_addr = match current_addr {
            SocketAddr::V4(_) => SocketAddr::new(IpAddr::from(ADDRESS_V6), PORT),
            SocketAddr::V6(_) => SocketAddr::new(IpAddr::from(ADDRESS_V4), PORT),
        };
        info!(
            old = %current_addr,
            new = %new_addr,
            "Amazon Time Sync source swapping address."
        );
        self.ntp.set_socket_address(new_addr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::{
        event::{NtpData, Stratum},
        time::{self, Instant, TscCount},
    };
    use rstest::{fixture, rstest};
    use std::io::{self, ErrorKind};

    async fn create_amazon_time_sync() -> (
        AmazonTimeSync,
        watch::Sender<ClockDisruptionEvent>,
        Arc<SelectedClockSource>,
        async_ring_buffer::Receiver<event::Ntp>,
    ) {
        let (event_sender, event_receiver) = async_ring_buffer::create::<event::Ntp>(1);
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent {
                disruption_marker: None,
            });

        let selected_clock = Arc::new(SelectedClockSource::default());
        (
            AmazonTimeSync::construct(
                event_sender,
                ctrl_receiver,
                clock_disruption_receiver,
                selected_clock.clone(),
            ),
            clock_disruption_sender,
            selected_clock,
            event_receiver,
        )
    }

    #[fixture]
    fn ntp_event() -> event::Ntp {
        event::Ntp::builder()
            .counter_pre(TscCount::new(1_000_000_000))
            .counter_post(TscCount::new(1_000_002_000))
            .ntp_data(NtpData {
                server_recv_time: Instant::from_days(1),
                server_send_time: Instant::from_days(1) + time::Duration::from_micros(1),
                root_delay: time::Duration::from_micros(10),
                root_dispersion: time::Duration::from_micros(20),
                stratum: Stratum::ONE,
            })
            .build()
            .unwrap()
    }

    fn routing_err() -> NtpIoError {
        NtpIoError::SampleIo(io::Error::from(ErrorKind::NetworkUnreachable))
    }

    fn v4() -> SocketAddr {
        SocketAddr::new(IpAddr::from(ADDRESS_V4), PORT)
    }

    fn v6() -> SocketAddr {
        SocketAddr::new(IpAddr::from(ADDRESS_V6), PORT)
    }

    #[test]
    fn address_v6_is_amazon_time_sync_endpoint() {
        assert_eq!(
            ADDRESS_V6,
            "fd00:ec2::123".parse::<Ipv6Addr>().unwrap(),
            "IPv6 address must be the Amazon Time Sync endpoint"
        );
        assert_eq!(
            ADDRESS_V4,
            "169.254.169.123".parse::<Ipv4Addr>().unwrap(),
            "IPv4 address must be the Amazon Time Sync endpoint"
        );
    }

    #[tokio::test]
    async fn swap_network_stack_toggles_family_and_keeps_port() {
        let (mut amazon_time_sync, _, _, _) = create_amazon_time_sync().await;

        amazon_time_sync.swap_network_stack();
        assert_eq!(amazon_time_sync.ntp.socket_address(), v6());

        amazon_time_sync.swap_network_stack();
        assert_eq!(amazon_time_sync.ntp.socket_address(), v4());
    }

    #[tokio::test]
    async fn routing_failures_below_threshold_do_not_swap() {
        let (mut amazon_time_sync, _, _, _) = create_amazon_time_sync().await;

        for _ in 0..MAX_CONSECUTIVE_FAILURE - 1 {
            amazon_time_sync.handle_ntp_sample_result(Err(routing_err()));
        }

        assert_eq!(amazon_time_sync.ntp.socket_address(), v4());
        assert_eq!(
            amazon_time_sync.consecutive_failure_count,
            MAX_CONSECUTIVE_FAILURE - 1
        );
    }

    #[rstest]
    #[case::bind(|| NtpIoError::Bind(io::Error::from(ErrorKind::NetworkUnreachable)))]
    #[case::sample_io(routing_err)]
    #[tokio::test]
    async fn routing_failures_at_threshold_swap_and_reset_counter(
        #[case] make_err: fn() -> NtpIoError,
    ) {
        let (mut amazon_time_sync, _, _, _) = create_amazon_time_sync().await;

        for _ in 0..MAX_CONSECUTIVE_FAILURE {
            amazon_time_sync.handle_ntp_sample_result(Err(make_err()));
        }

        assert_eq!(amazon_time_sync.ntp.socket_address(), v6());
        assert_eq!(amazon_time_sync.consecutive_failure_count, 0);
    }

    #[tokio::test]
    async fn timeouts_never_swap() {
        let (mut amazon_time_sync, _, _, _) = create_amazon_time_sync().await;

        for _ in 0..MAX_CONSECUTIVE_FAILURE * 3 {
            // `time::error::Elapsed` has a private constructor, so obtain one by
            // actually timing out a pending future.
            let elapsed =
                tokio::time::timeout(Duration::from_nanos(1), futures::future::pending::<()>())
                    .await
                    .unwrap_err();
            amazon_time_sync.handle_ntp_sample_result(Err(NtpIoError::Timeout(elapsed)));
        }

        assert_eq!(amazon_time_sync.ntp.socket_address(), v4());
        assert_eq!(amazon_time_sync.consecutive_failure_count, 0);
    }

    #[tokio::test]
    async fn tsc_order_errors_never_swap() {
        let (mut amazon_time_sync, _, _, _) = create_amazon_time_sync().await;

        for _ in 0..MAX_CONSECUTIVE_FAILURE * 3 {
            amazon_time_sync
                .handle_ntp_sample_result(Err(NtpIoError::TscOrder { pre: 2, post: 1 }));
        }

        assert_eq!(amazon_time_sync.ntp.socket_address(), v4());
        assert_eq!(amazon_time_sync.consecutive_failure_count, 0);
    }

    #[rstest]
    #[tokio::test]
    async fn success_resets_failure_count(ntp_event: event::Ntp) {
        let (mut amazon_time_sync, _, _, _event_receiver) = create_amazon_time_sync().await;

        for _ in 0..MAX_CONSECUTIVE_FAILURE - 1 {
            amazon_time_sync.handle_ntp_sample_result(Err(routing_err()));
        }
        amazon_time_sync.handle_ntp_sample_result(Ok(ntp_event));
        for _ in 0..MAX_CONSECUTIVE_FAILURE - 1 {
            amazon_time_sync.handle_ntp_sample_result(Err(routing_err()));
        }

        assert_eq!(amazon_time_sync.ntp.socket_address(), v4());
    }

    #[rstest]
    #[tokio::test]
    async fn success_forwards_event_to_ring_buffer(ntp_event: event::Ntp) {
        let (mut amazon_time_sync, _, _, event_receiver) = create_amazon_time_sync().await;

        amazon_time_sync.handle_ntp_sample_result(Ok(ntp_event.clone()));

        let received = event_receiver.recv().await.unwrap();
        assert_eq!(received, ntp_event);
    }

    #[tokio::test]
    async fn swaps_back_to_ipv4_after_ipv6_failures() {
        let (mut amazon_time_sync, _, _, _) = create_amazon_time_sync().await;

        for _ in 0..MAX_CONSECUTIVE_FAILURE * 2 {
            amazon_time_sync.handle_ntp_sample_result(Err(routing_err()));
        }

        assert_eq!(amazon_time_sync.ntp.socket_address(), v4());
    }

    #[tokio::test]
    async fn validate_to_burst_mode() {
        let (mut amazon_time_sync, clock_disruption_sender, _, _) = create_amazon_time_sync().await;

        // Send a disruption signal
        clock_disruption_sender
            .send(ClockDisruptionEvent {
                disruption_marker: Some(1),
            })
            .unwrap();

        amazon_time_sync.ntp.handle_disruption();

        let branches = amazon_time_sync.ntp.select_branches();
        assert_eq!(branches.interval.period(), BURST_INTERVAL,);
    }

    #[tokio::test(start_paused = true)]
    async fn validate_to_normal_mode() {
        let (mut amazon_time_sync, clock_disruption_sender, _, _) = create_amazon_time_sync().await;

        // Send a disruption signal to enter burst mode
        clock_disruption_sender
            .send(ClockDisruptionEvent {
                disruption_marker: Some(1),
            })
            .unwrap();

        amazon_time_sync.ntp.handle_disruption();

        // Verify we're in burst mode
        let branches = amazon_time_sync.ntp.select_branches();
        assert_eq!(branches.interval.period(), BURST_INTERVAL);

        // Advance time past the burst duration to trigger expiry
        tokio::time::advance(BURST_DURATION + Duration::from_millis(1)).await;

        // handle_burst_expiry should detect elapsed burst and transition to normal
        assert!(amazon_time_sync.ntp.handle_burst_expiry());

        // Verify the interval is now the normal interval
        let branches = amazon_time_sync.ntp.select_branches();
        assert_eq!(branches.interval.period(), INTERVAL_DURATION);
    }
}
