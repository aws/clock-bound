//! Ntp IO Sources

use std::net::{Ipv4Addr, SocketAddrV4};

use thiserror::Error;
use tokio::{
    io,
    net::UdpSocket,
    sync::mpsc,
    time::{self, Duration, Interval, interval, timeout},
};
use tracing::{debug, info};

use super::CommunicationChannels;
use super::tsc::read_timestamp_counter;
use crate::daemon::{
    event::{self, NtpData},
    time::tsc::TscCount,
};

pub mod packet;
pub use packet::Packet;

pub const UNSPECIFIED_SOCKET_ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0);
const LINK_LOCAL_ADDRESS: SocketAddrV4 = SocketAddrV4::new(Ipv4Addr::new(169, 254, 169, 123), 123);
const INTERVAL_DURATION: Duration = Duration::from_secs(1);
const LINK_LOCAL_TIMEOUT: Duration = Duration::from_millis(100);

#[derive(Debug, Error)]
pub enum LinkLocalError {
    #[error("IO failure.")]
    Io(#[from] io::Error),
    #[error("Failed to parse NTP packet.")]
    PacketParsing(String),
    #[error("Send NtpEvent message failed.")]
    SendEventMessage(#[from] mpsc::error::SendError<event::Ntp>),
    #[error("Operation timed out.")]
    Timeout(#[from] time::error::Elapsed),
    #[error("TSC order failure. tsc_pre: {pre}. tsc_post: {post}")]
    TscOrder { pre: u64, post: u64 },
}

/// Contains the data needed to run the link local runner.
#[derive(Debug)]
pub struct LinkLocal {
    socket: UdpSocket,
    communication_channels: CommunicationChannels,
    ntp_buffer: [u8; Packet::SIZE],
    interval: Interval,
}

impl LinkLocal {
    /// Constructs a new `LinkLocal` with using given parameters.
    pub fn construct(socket: UdpSocket, communication_channels: CommunicationChannels) -> Self {
        LinkLocal {
            socket,
            communication_channels,
            ntp_buffer: [0u8; Packet::SIZE],
            interval: interval(INTERVAL_DURATION),
        }
    }

    /// Samples the Link local source.
    ///
    /// When sampling from a NTP source we first collect the current time stamp counter
    /// value. We then send a NTP request and await for a response. Once we receive a
    /// response we again collect the current time stamp counter value. After we've
    /// collected the NTP sample we construct the `Event` and push that event through
    /// to the ring buffer.
    async fn sample(&mut self) -> Result<(), LinkLocalError> {
        let packet = Packet::new_request(0);
        packet.emit_bytes(&mut self.ntp_buffer);

        // TODO: tsc reads and ntp samples need to be fenced.
        // We are currently investigating how to implement this appropriately.
        let sent_timestamp = read_timestamp_counter();

        // Request and Receive NTP sample.
        let recv_packet_result = timeout(LINK_LOCAL_TIMEOUT, {
            self.socket
                .send_to(&self.ntp_buffer, LINK_LOCAL_ADDRESS)
                .await?;
            self.socket.recv_from(&mut self.ntp_buffer)
        })
        .await?;
        let received_timestamp = read_timestamp_counter();

        let (_, ntp_packet) = Packet::parse_from_bytes(&self.ntp_buffer)
            .map_err(|e| LinkLocalError::PacketParsing(e.to_string()))?;

        let ntp_data = NtpData::try_from(ntp_packet)
            .map_err(|e| LinkLocalError::PacketParsing(e.to_string()))?;

        let ntp_event = event::Ntp::builder()
            .tsc_pre(TscCount::new(sent_timestamp.into()))
            .tsc_post(TscCount::new(received_timestamp.into()))
            .ntp_data(ntp_data)
            .build()
            .ok_or(LinkLocalError::TscOrder {
                pre: sent_timestamp,
                post: received_timestamp,
            })?;

        debug!(?recv_packet_result, "Received packet.");
        self.communication_channels
            .event_sender
            .send(ntp_event.clone())
            .await?;
        debug!(?ntp_event, "Successfully send link local event.");
        Ok(())
    }

    /// NTP Link Local task runner.
    ///
    /// Sampling NTP packets from the AWS EC2 internal Link Local address.
    ///
    /// # Panics
    /// Function will panic if not called within the `tokio` runtime.
    pub async fn run(&mut self) {
        // Sampling loop
        info!("Starting link local sampling loop.");
        loop {
            tokio::select! {
                _ = self.interval.tick() => {
                    if let Err(e) = self.sample().await {
                        debug!(?e, "Failed to sample link local source.");
                    }
                }
                _ = self.communication_channels.ctrl_receiver.recv() => {
                    // Ctrl logic here.
                    // Currently we breakout of the loop if we receive a control event.
                    break;
                }
                _ = self.communication_channels.clock_disruption_receiver.as_mut().unwrap().changed() => {
                    // Clock Disruption logic here
                    todo!("Clock disruption logic has yet to be implemented.");
                }
            }
        }
        info!("Link local runner exiting.");
    }
}
