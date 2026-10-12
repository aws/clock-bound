//! Ntp IO Source constants and base struct

use std::{
    io::{self, ErrorKind},
    net::{Ipv4Addr, Ipv6Addr, SocketAddr},
    num::Wrapping,
    sync::Arc,
};
use thiserror::Error;
use tokio::{
    net::UdpSocket,
    sync::{mpsc, watch},
    time::{self, Duration, Instant, Interval, MissedTickBehavior, interval_at},
};
use tracing::{debug, trace, warn};

pub mod packet;
pub use packet::{ExtensionField, Fec2V1Value as DaemonInfo, Packet};

use crate::daemon::{
    async_ring_buffer::{self, BufferClosedError, SendError},
    event::{self, NtpData},
    io::{
        ClockDisruptionEvent, ControlRequest,
        tsc::{read_timestamp_counter_begin, read_timestamp_counter_end},
    },
    selected_clock::SelectedClockSource,
    time::TscCount,
};

use packet::Timestamp;

/// Unified error type for all NTP IO sources.
#[derive(Debug, Error)]
pub enum NtpIoError {
    #[error("Sample IO failure.")]
    SampleIo(#[source] io::Error),
    #[error("Socket bind failure")]
    Bind(#[source] io::Error),
    #[error("Operation timed out.")]
    Timeout(#[from] time::error::Elapsed),
    #[error("TSC order failure. tsc_pre: {pre}. tsc_post: {post}")]
    TscOrder { pre: u64, post: u64 },
}

impl NtpIoError {
    /// Returns `true` if this error indicates the remote NTP source is unreachable
    /// and should be retired so a replacement can be selected.
    ///
    /// Indicates errors to the `bind(2)`, `send_to(2)`, and `recv_from(2)` mean that
    /// the source is unreachable and should be replaced.
    ///
    /// This gates reselection on more than timeouts. A host's network stack can change
    /// underneath us: an IPv6 route can disappear, IPv6 can be disabled entirely, or an
    /// address can stop being locally available. When that happens the local `bind` or the
    /// `send_to`/`recv_from` fails with a routing or address-family error rather than timing
    /// out. Treating those as "unreachable" lets the DNS resolver quarantine the dead address
    /// and reselect a reachable one, so the daemon stays resilient across network-stack changes
    /// instead of pinning a permanently-broken source.
    ///
    /// Everything else is treated as a transient local failure: it is retried on the next tick
    /// and never retires the source, so a transient kernel hiccup cannot thrash the whole pool.
    pub fn indicates_unreachable_source(&self) -> bool {
        match self {
            NtpIoError::Timeout(_) => true,
            NtpIoError::Bind(e) | NtpIoError::SampleIo(e) => is_unreachable_io_error(e),
            NtpIoError::TscOrder { .. } => false,
        }
    }
}

/// Classifies an [`io::Error`] from `bind`/`send_to`/`recv_from` as a routing or
/// address-family failure that means this destination is unreachable on the current host.
///
/// returning true here means that the remote endpoint is unreachable and should
/// eventually be re-resolved
fn is_unreachable_io_error(error: &io::Error) -> bool {
    // Routing / addressing failures that carry a stable `ErrorKind`.
    //
    // NetworkUnreachable: Local route unreachable. Could happen if an IPv6 route disappears.
    // HostUnreachable: Local route exists but the host is down.
    // AddrNotAvailable: Local address is not available (e.g. interface down).
    if matches!(
        error.kind(),
        ErrorKind::NetworkUnreachable | ErrorKind::HostUnreachable | ErrorKind::AddrNotAvailable
    ) {
        return true;
    }

    // Address-family mismatches have no stable `ErrorKind`, so match the raw errno.
    // EAFNOSUPPORT: address family not supported (e.g. an IPv6 dest on an IPv4-only stack).
    // EINVAL: the kernel rejected the send as invalid for this socket's family.
    #[cfg(unix)]
    if matches!(
        error.raw_os_error(),
        Some(libc::EAFNOSUPPORT | libc::EINVAL)
    ) {
        return true;
    }

    false
}

/// Returns the address-family-matched wildcard bind address for a destination.
///
/// Some systems enable `IPV6_V6ONLY` by default, so a single unspecified socket
/// cannot serve both families. Bind the wildcard address matching the destination's
/// family instead.
fn wildcard_bind_addr(destination: SocketAddr) -> SocketAddr {
    match destination {
        SocketAddr::V4(_) => (Ipv4Addr::UNSPECIFIED, 0).into(),
        SocketAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    }
}

/// Base NTP IO struct holding common components shared by all NTP-based IO sources.
///
/// Each concrete source (e.g., `IpAddrSource`, `AmazonTimeSync`) composes this struct
/// and delegates common IO plumbing to it.
///
/// Optionally manages burst mode internally when constructed with a [`BurstConfig`].
/// In that case, the struct starts in burst mode, enters burst mode on clock disruptions,
/// and transitions back to normal mode after the configured duration elapses.
#[derive(Debug)]
pub struct Ntp {
    event_sender: async_ring_buffer::Sender<event::Ntp>,
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    interval: Interval,
    normal_interval: Duration,
    buffer: Vec<u8>,
    selected_clock: Arc<SelectedClockSource>,
    transmit_counter: Wrapping<u64>,
    extensions: Vec<ExtensionField>,
    socket_address: SocketAddr,
    timeout: Duration,
    burst: Option<BurstState>,
}

#[bon::bon]
impl Ntp {
    /// Constructs a new `Ntp` base struct.
    ///
    /// # Panics
    ///
    /// Panics if not constructed within a tokio runtime
    #[builder]
    pub fn new(
        /// Ring buffer sender for NTP events
        event_sender: async_ring_buffer::Sender<event::Ntp>,
        /// Channel for receiving control requests (shutdown)
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        /// Watch channel for clock disruption signals
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        /// Shared reference to the currently selected clock source
        selected_clock: Arc<SelectedClockSource>,
        /// Extension fields to include in outgoing NTP packets
        extensions: Vec<ExtensionField>,
        /// Target NTP server socket address
        socket_address: SocketAddr,
        /// Per-sample timeout duration
        timeout: Duration,
        /// Normal polling interval duration
        interval: Duration,
        /// Optional burst mode configuration. When provided, the source starts
        /// in burst mode and manages burst/normal transitions internally.
        burst_config: Option<BurstConfig>,
    ) -> Self {
        let buffer_size = Packet::MIN_SIZE
            + extensions
                .iter()
                .map(|ext| ext.length() as usize)
                .sum::<usize>();

        let initial_interval = burst_config.as_ref().map_or(interval, |bc| bc.interval);
        let mut ntp_interval = tokio::time::interval(initial_interval);
        ntp_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let burst = burst_config.map(|config| BurstState {
            config,
            mode: Mode::burst(),
        });

        let mut retval = Ntp {
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            interval: ntp_interval,
            normal_interval: interval,
            buffer: vec![0u8; buffer_size],
            selected_clock,
            transmit_counter: Wrapping(0),
            extensions,
            socket_address,
            timeout,
            burst,
        };
        retval.decorrelate_poll_timing();
        retval
    }
}

impl Ntp {
    /// Returns a [`SelectBranches`] struct providing simultaneous mutable access to the
    /// clock disruption receiver, control receiver, and interval for use in a `tokio::select!`.
    pub fn select_branches(&mut self) -> SelectBranches<'_> {
        SelectBranches {
            clock_disruption_receiver: &mut self.clock_disruption_receiver,
            ctrl_receiver: &mut self.ctrl_receiver,
            interval: &mut self.interval,
        }
    }

    /// Set the interval to fire at a random time in the future
    ///
    /// This prevents all NTP tasks from polling at the same time
    fn decorrelate_poll_timing(&mut self) {
        let max_delay = if let Some(burst) = &self.burst
            && let Mode::Burst(_) = &burst.mode
        {
            burst.config.interval.as_nanos() as usize
        } else {
            self.normal_interval.as_nanos() as usize
        };
        let delay = rand::random_range(0..=max_delay) as u64;
        let delay = Duration::from_nanos(delay);
        self.interval.reset_after(delay);
    }

    /// Return the socket address
    pub fn socket_address(&self) -> SocketAddr {
        self.socket_address
    }

    /// Samples the NTP source by building a packet, sending it, and awaiting a response.
    ///
    /// Handles counter management, packet construction with configured extensions,
    /// socket binding, and the NTP request/response cycle.
    ///
    /// # Errors
    /// Returns an error if the socket cannot be bound, the operation times out,
    /// or the TSC counter ordering is invalid.
    pub async fn sample(&mut self) -> Result<event::Ntp, NtpIoError> {
        let counter = self.transmit_counter.0;
        self.transmit_counter += 1;
        let (source, stratum) = self.selected_clock.get_with_client_stratum();
        let packet = Packet::builder()
            .transmit_timestamp(Timestamp::new(counter))
            .stratum(stratum.into())
            .reference_id(source.into())
            .extensions(self.extensions.clone())
            .build();
        packet.emit_bytes(&mut self.buffer);

        let bind_address = wildcard_bind_addr(self.socket_address);

        let socket = UdpSocket::bind(bind_address)
            .await
            .map_err(NtpIoError::Bind)?;

        let ntp_event = sample_packet(
            &socket,
            self.socket_address,
            &mut self.buffer,
            self.timeout,
            counter,
        )
        .await
        .inspect_err(|error| {
            if let NtpIoError::TscOrder { .. } = error {
                warn!(
                    ?error,
                    "TSC event order mismatch detected. Dropping sample."
                );
            }
        })?;
        debug!(?ntp_event, "Received packet.");
        Ok(ntp_event)
    }

    /// Handles a clock disruption event.
    ///
    /// Checks whether a disruption marker is present. If so, notifies the event sender
    /// to clear its buffer and enters burst mode (if burst is configured).
    /// Returns `true` if a disruption was present, `false` otherwise.
    pub fn handle_disruption(&mut self) -> bool {
        // Destructure all fields so adding a new field forces a compiler error here.
        let Self {
            clock_disruption_receiver,
            event_sender,
            ctrl_receiver: _,
            interval,
            normal_interval: _,
            buffer: _,
            selected_clock: _,
            transmit_counter: _,
            extensions: _,
            socket_address: _,
            timeout: _,
            burst,
        } = self;
        let val = clock_disruption_receiver.borrow_and_update().clone();
        if val.disruption_marker.is_some() {
            event_sender.handle_disruption();
            if let Some(burst_state) = burst {
                burst_state.mode = Mode::burst();
                *interval = tokio::time::interval(burst_state.config.interval);
                interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
                interval.reset_immediately();
            }
            return true;
        }
        false
    }

    /// Checks whether burst mode has expired and transitions to normal mode if so.
    ///
    /// Call this after a successful sample. If burst mode is not configured or the source
    /// is already in normal mode, this is a no-op. Returns `true` if a transition occurred.
    pub fn handle_burst_expiry(&mut self) -> bool {
        let Some(burst_state) = &mut self.burst else {
            return false;
        };
        if let Mode::Burst(start_time) = burst_state.mode
            && start_time.elapsed() >= burst_state.config.duration
        {
            burst_state.mode = Mode::Normal;
            let normal = self.normal_interval;
            self.interval = interval_at(Instant::now() + normal, normal);
            self.interval
                .set_missed_tick_behavior(MissedTickBehavior::Delay);
            self.decorrelate_poll_timing();
            return true;
        }
        false
    }

    /// Sends an NTP event through the ring buffer.
    ///
    /// Silently drops the event if a disruption occurs. This will be handled by the task
    /// on the next iteration of the IO task loop
    ///
    /// # Errors
    ///
    /// Returns an error if the buffer receiver is closed
    pub fn send_event(&self, event: &event::Ntp) -> Result<(), BufferClosedError> {
        match self.event_sender.send(event.clone()) {
            Ok(()) => {
                debug!(?event, "Successfully sent IO event.");
                Ok(())
            }
            Err(SendError::Disrupted(_)) => {
                // next await of this task will respond with a disruption notification
                debug!("Trying to send when there was a disruption event.");
                Ok(())
            }
            Err(SendError::BufferClosed(e)) => Err(e),
        }
    }
}

/// Configuration for burst mode behavior.
///
/// When provided to an NTP source, enables burst mode on startup and after clock disruptions.
/// During burst mode, the source polls at a higher frequency for the specified duration
/// before transitioning back to normal mode.
#[derive(Debug, Clone)]
pub struct BurstConfig {
    /// Duration of burst mode before transitioning to normal.
    pub duration: Duration,
    /// Polling interval while in burst mode.
    pub interval: Duration,
}

/// An enum indicating the interval state of the source IO.
///
/// # Variants:
/// - `Normal` mode indicates that the source is sampling at a constant frequency and remains so
///   unless given an external signal.
/// - `Burst` mode indicates that the source is in a temporary mode during which the underlying
///   source is polled more frequently.
#[derive(Debug)]
enum Mode {
    /// Indicates that the source is in its normal operating mode.
    Normal,
    /// Indicates that the source should be in burst mode and when it entered burst mode.
    Burst(Instant),
}

impl Mode {
    /// Constructs the [`Mode::Burst`] variant, capturing the current instant as the start time.
    fn burst() -> Mode {
        Mode::Burst(Instant::now())
    }
}

/// Named struct providing simultaneous mutable access to the channels and interval
/// needed by a `tokio::select!` loop without breaking the borrow checker.
#[derive(Debug)]
pub struct SelectBranches<'a> {
    pub clock_disruption_receiver: &'a mut watch::Receiver<ClockDisruptionEvent>,
    pub ctrl_receiver: &'a mut mpsc::Receiver<ControlRequest>,
    pub interval: &'a mut Interval,
}

/// Internal burst state, present only when a [`BurstConfig`] was provided at construction.
#[derive(Debug)]
struct BurstState {
    config: BurstConfig,
    mode: Mode,
}

/// Sample an NTP event
///
/// Send an NTP request and return the response
///
/// This function will effectively loop over:
/// - receiving a packet
/// - parsing it
/// - ensuring the origin timestamp matches the one that was sent
/// - converting the packet into an [`event::Ntp`]
///
/// # Errors
/// Returns an error if:
/// - the socket has an IO error
/// - the transaction times out
async fn sample_packet(
    socket: &UdpSocket,
    addr: SocketAddr,
    send_recv_buffer: &mut [u8],
    timeout: std::time::Duration,
    expected_counter: u64,
) -> Result<event::Ntp, NtpIoError> {
    let fut = tokio::time::timeout(
        timeout,
        inner_timeout(socket, addr, send_recv_buffer, expected_counter),
    );

    let (send_timestamp, ntp_data, received_timestamp) =
        fut.await?.map_err(NtpIoError::SampleIo)?;

    #[cfg(not(test))]
    let system_clock_reading = crate::daemon::event::SystemClockMeasurement::now();

    #[allow(clippy::cast_possible_wrap)]
    let tsc_pre = TscCount::new(send_timestamp as i64);
    #[allow(clippy::cast_possible_wrap)]
    let tsc_post = TscCount::new(received_timestamp as i64);
    let builder = event::Ntp::builder()
        .counter_pre(tsc_pre)
        .counter_post(tsc_post)
        .ntp_data(ntp_data);

    let ntp_event = {
        #[cfg(not(test))]
        {
            builder.system_clock(system_clock_reading).build()
        }
        #[cfg(test)]
        {
            builder.build()
        }
    };

    let ntp_event = ntp_event.ok_or(NtpIoError::TscOrder {
        pre: send_timestamp,
        post: received_timestamp,
    })?;

    Ok(ntp_event)
}

// private inner function which loops indefinitely. Meant to be wrapped in a timeout
//
// Returns the tx tsc, NTP data, and the rx tsc
async fn inner_timeout(
    socket: &UdpSocket,
    addr: SocketAddr,
    send_recv_buffer: &mut [u8],
    expected_counter: u64,
) -> Result<(u64, NtpData, u64), io::Error> {
    let send_timestamp = read_timestamp_counter_begin();
    socket.send_to(send_recv_buffer, addr).await?;
    loop {
        let (len, recv_addr) = socket.recv_from(send_recv_buffer).await?;
        let received_timestamp = read_timestamp_counter_end();
        if recv_addr != addr {
            continue;
        }
        let Ok((_, ntp_packet)) = Packet::parse_from_bytes(&send_recv_buffer[..len])
            .inspect_err(|e| trace!(parse_error = ?e.to_string(), "Parsing error."))
        else {
            continue;
        };

        if ntp_packet.origin_timestamp.get() != expected_counter {
            trace!(
                error = ?InnerSamplePacketError::OriginMismatch {
                    expected: expected_counter,
                    received: ntp_packet.origin_timestamp.get(),
                },
                "Origin timestamp mismatch."
            );
            continue;
        }

        let Ok(ntp_data) = NtpData::try_from(ntp_packet)
            .inspect_err(|e| trace!(error = ?InnerSamplePacketError::PacketParsing(e.to_string()), "Failed to parse NTP data.")) else {
                continue;
            };

        return Ok((send_timestamp, ntp_data, received_timestamp));
    }
}

#[derive(Debug, thiserror::Error)]
enum InnerSamplePacketError {
    #[error("IO failure.")]
    Io(#[from] io::Error),
    #[error("Failed to parse NTP packet.")]
    PacketParsing(String),
    #[error("Mismatched origin. Expected {expected}, got {received}")]
    OriginMismatch { expected: u64, received: u64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // Routing/addressing failures are unreachable; transient and unrelated kinds are not.
    #[rstest]
    #[case::network_unreachable(ErrorKind::NetworkUnreachable, true)]
    #[case::host_unreachable(ErrorKind::HostUnreachable, true)]
    #[case::addr_not_available(ErrorKind::AddrNotAvailable, true)]
    #[case::connection_refused(ErrorKind::ConnectionRefused, false)]
    #[case::would_block(ErrorKind::WouldBlock, false)]
    #[case::permission_denied(ErrorKind::PermissionDenied, false)]
    #[case::other(ErrorKind::Other, false)]
    fn is_unreachable_io_error_classifies_error_kinds(
        #[case] kind: ErrorKind,
        #[case] expected: bool,
    ) {
        let error = io::Error::from(kind);
        assert_eq!(
            is_unreachable_io_error(&error),
            expected,
            "ErrorKind {kind:?} classified wrong"
        );
    }

    // Address-family mismatches surface as raw errnos with no stable `ErrorKind`.
    #[cfg(unix)]
    #[rstest]
    #[case::af_not_supported(libc::EAFNOSUPPORT)]
    #[case::invalid(libc::EINVAL)]
    fn is_unreachable_io_error_classifies_errnos(#[case] errno: i32) {
        let error = io::Error::from_raw_os_error(errno);
        assert!(
            is_unreachable_io_error(&error),
            "errno {errno} should be classified as unreachable"
        );
    }

    #[test]
    fn indicates_unreachable_source_for_bind_errors() {
        let unreachable = NtpIoError::Bind(io::Error::from(ErrorKind::NetworkUnreachable));
        assert!(
            unreachable.indicates_unreachable_source(),
            "Bind with NetworkUnreachable should indicate an unreachable source"
        );

        let transient = NtpIoError::Bind(io::Error::from(ErrorKind::ConnectionRefused));
        assert!(
            !transient.indicates_unreachable_source(),
            "Bind with ConnectionRefused should not indicate an unreachable source"
        );
    }

    #[test]
    fn indicates_unreachable_source_for_sample_io_errors() {
        let unreachable = NtpIoError::SampleIo(io::Error::from(ErrorKind::NetworkUnreachable));
        assert!(
            unreachable.indicates_unreachable_source(),
            "SampleIo with NetworkUnreachable should indicate an unreachable source"
        );

        let transient = NtpIoError::SampleIo(io::Error::from(ErrorKind::ConnectionRefused));
        assert!(
            !transient.indicates_unreachable_source(),
            "SampleIo with ConnectionRefused should not indicate an unreachable source"
        );
    }

    #[test]
    fn indicates_unreachable_source_for_tsc_order() {
        let error = NtpIoError::TscOrder { pre: 2, post: 1 };
        assert!(
            !error.indicates_unreachable_source(),
            "TscOrder is a local failure and should not indicate an unreachable source"
        );
    }

    #[tokio::test]
    async fn indicates_unreachable_source_for_timeout() {
        // `time::error::Elapsed` has a private constructor, so obtain one by
        // actually timing out a pending future.
        let elapsed =
            tokio::time::timeout(Duration::from_nanos(1), futures::future::pending::<()>())
                .await
                .unwrap_err();
        let error = NtpIoError::Timeout(elapsed);
        assert!(
            error.indicates_unreachable_source(),
            "Timeout should indicate an unreachable source"
        );
    }

    #[test]
    fn wildcard_bind_addr_matches_v4_destination() {
        let dest: SocketAddr = "1.2.3.4:123".parse().unwrap();
        let addr = wildcard_bind_addr(dest);
        assert_eq!(
            addr,
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
            "V4 destination should produce the IPv4 wildcard address"
        );
        assert!(addr.is_ipv4(), "V4 destination should bind an IPv4 address");
    }

    #[test]
    fn wildcard_bind_addr_matches_v6_destination() {
        let dest: SocketAddr = "[2001:db8::1]:123".parse().unwrap();
        let addr = wildcard_bind_addr(dest);
        assert_eq!(
            addr,
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
            "V6 destination should produce the IPv6 wildcard address"
        );
        assert!(addr.is_ipv6(), "V6 destination should bind an IPv6 address");
    }
}
