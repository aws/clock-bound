//! Ntp IO Source constants

use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};
use tokio::{net::UdpSocket, time, time::Duration};

pub mod packet;
pub use packet::{Fec2V1Value as DaemonInfo, Packet};

pub mod socket_ext;

use crate::daemon::{
    async_ring_buffer,
    event::{self, NtpData},
    io::{
        ntp::socket_ext::SocketExt,
        tsc::{read_timestamp_counter_begin, read_timestamp_counter_end},
    },
    time::TscCount,
};

pub const AMAZON_TIME_SYNC_BURST_DURATION: Duration = Duration::from_secs(1);
/// The amount of time between source polls when in burst mode.
pub const AMAZON_TIME_SYNC_BURST_INTERVAL_DURATION: Duration = Duration::from_millis(50);

pub const UNSPECIFIED_SOCKET_ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
pub const AMAZON_TIME_SYNC_ADDRESS: SocketAddrV4 =
    SocketAddrV4::new(Ipv4Addr::new(169, 254, 169, 123), 123);
pub const AMAZON_TIME_SYNC_INTERVAL_DURATION: Duration = Duration::from_secs(2);
pub const AMAZON_TIME_SYNC_TIMEOUT: Duration = Duration::from_millis(100);

pub const NTP_SOURCE_INTERVAL_DURATION: Duration = Duration::from_secs(16);
pub const NTP_SOURCE_TIMEOUT: Duration = Duration::from_millis(100);

/// Tuple to hold both the `SocketAddr` and ring buffer `Sender` for an IO `NTPSource`
pub type NTPSourceSender = (SocketAddr, async_ring_buffer::Sender<event::Ntp>);
/// Tuple to hold both the `SocketAddr` and ring buffer `Receiver` for an IO `NTPSource`
pub type NTPSourceReceiver = (SocketAddr, async_ring_buffer::Receiver<event::Ntp>);

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
pub async fn sample_packet(
    socket: &UdpSocket,
    addr: SocketAddr,
    send_recv_buffer: &mut [u8],
    timeout: std::time::Duration,
    expected_counter: u64,
) -> Result<event::Ntp, SamplePacketError> {
    socket.clear().map_err(SamplePacketError::SocketClear)?;
    let fut = tokio::time::timeout(
        timeout,
        inner_timeout(socket, addr, send_recv_buffer, expected_counter),
    );

    let (send_timestamp, ntp_data, received_timestamp) = fut.await??;

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

    let ntp_event = ntp_event.ok_or(SamplePacketError::TscOrder {
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
            .inspect_err(|e| tracing::trace!(parse_error = ?e.to_string(), "Parsing error"))
        else {
            continue;
        };

        if ntp_packet.origin_timestamp.get() != expected_counter {
            tracing::trace!(error = ?InnerSamplePacketError::OriginMismatch {
                expected: expected_counter,
                received: ntp_packet.origin_timestamp.get(),
            });
            continue;
        }

        let Ok(ntp_data) = NtpData::try_from(ntp_packet)
            .inspect_err(|e| tracing::trace!(error = ?InnerSamplePacketError::PacketParsing(e.to_string()), "NtpData try from error")) else {
                continue;
            };

        return Ok((send_timestamp, ntp_data, received_timestamp));
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SamplePacketError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Timeout(#[from] time::error::Elapsed),
    #[error("TSC order failure. tsc_pre: {pre}. tsc_post: {post}")]
    TscOrder { pre: u64, post: u64 },
    #[error("IO failure on socket clear")]
    SocketClear(#[source] io::Error),
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
