//! NTP Server IO Source

use std::{net::SocketAddr, sync::Arc};
use thiserror::Error;
use tokio::{
    io,
    net::UdpSocket,
    sync::{mpsc, watch},
    time::{self, Interval, MissedTickBehavior, interval, timeout},
};
use tracing::{debug, info};

use super::tsc::{read_timestamp_counter_begin, read_timestamp_counter_end};
use crate::daemon::{
    async_ring_buffer,
    event::{self, NtpData},
    io::{
        ClockDisruptionEvent, ControlRequest, DaemonInfo,
        ntp::packet::{ExtensionField, Timestamp},
    },
    selected_clock::SelectedClockSource,
    time::tsc::TscCount,
};

use super::ntp::{NTP_SOURCE_INTERVAL_DURATION, NTP_SOURCE_TIMEOUT, packet};
use packet::Packet;

#[derive(Debug, Error)]
pub enum NTPSourceError {
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

/// Contains data used to run `NTPSource` runner.
/// Notably, the IP address passed to this struct should be associated
/// with an NTP host.
#[derive(Debug)]
pub struct NTPSource {
    socket: UdpSocket,
    address: SocketAddr,
    event_sender: async_ring_buffer::Sender<event::Ntp>,
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    ntp_buffer: [u8; Packet::MIN_SIZE + ExtensionField::MIN_SIZE],
    interval: Interval,
    selected_clock: Arc<SelectedClockSource>,
    daemon_info: DaemonInfo,
}

impl NTPSource {
    /// Constructs a new `NTPSource` with using given parameters.
    pub fn construct(
        socket: UdpSocket,
        address: SocketAddr,
        event_sender: async_ring_buffer::Sender<event::Ntp>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        selected_clock: Arc<SelectedClockSource>,
        daemon_info: DaemonInfo,
    ) -> Self {
        let mut ntp_source_interval = interval(NTP_SOURCE_INTERVAL_DURATION);
        ntp_source_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        NTPSource {
            socket,
            address,
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            ntp_buffer: [0u8; Packet::MIN_SIZE + ExtensionField::MIN_SIZE],
            interval: ntp_source_interval,
            selected_clock,
            daemon_info,
        }
    }

    /// Samples the NTP Source source.
    ///
    /// When sampling from a NTP source we first collect the current time stamp counter
    /// value. We then send a NTP request and await for a response. Once we receive a
    /// response we again collect the current time stamp counter value. After we've
    /// collected the NTP sample we construct the `Event` and push that event through
    /// to the ring buffer.
    async fn sample(&mut self) -> Result<event::Ntp, NTPSourceError> {
        let (refid, stratum) = self.selected_clock.get();
        let packet = Packet::builder()
            .transmit_timestamp(Timestamp::new(0))
            .stratum(stratum.incremented().into())
            .reference_id(refid.into())
            .extensions(vec![ExtensionField::Fec2V1(self.daemon_info.clone())])
            .build();
        packet.emit_bytes(&mut self.ntp_buffer);

        let sent_timestamp = read_timestamp_counter_begin();

        // Request and Receive NTP sample.
        let recv_packet_result = timeout(NTP_SOURCE_TIMEOUT, {
            let packet_size = packet.total_size();
            self.socket
                .send_to(&self.ntp_buffer[..packet_size], self.address)
                .await?;
            self.socket.recv_from(&mut self.ntp_buffer)
        })
        .await?;

        let received_timestamp = read_timestamp_counter_end();

        let (_, ntp_packet) = Packet::parse_from_bytes(&self.ntp_buffer)
            .map_err(|e| NTPSourceError::PacketParsing(e.to_string()))?;

        let ntp_data = NtpData::try_from(ntp_packet)
            .map_err(|e| NTPSourceError::PacketParsing(e.to_string()))?;

        let ntp_event = event::Ntp::builder()
            .tsc_pre(TscCount::new(sent_timestamp.into()))
            .tsc_post(TscCount::new(received_timestamp.into()))
            .ntp_data(ntp_data)
            .build()
            .ok_or(NTPSourceError::TscOrder {
                pre: sent_timestamp,
                post: received_timestamp,
            })?;

        debug!(?recv_packet_result, "Received packet.");
        Ok(ntp_event)
    }

    /// `NTPSource` task runner.
    ///
    /// Sampling NTP packets from the IP Address defined at initialization.
    ///
    /// # Panics
    /// Function will panic if not called within the `tokio` runtime.
    ///
    /// # Errors
    /// Returns error if loop exits unexpectedly
    pub async fn run(&mut self) -> Result<(), NTPSourceError> {
        // Sampling loop
        info!("Starting NTP Source IO sampling loop.");
        loop {
            tokio::select! {
                _ = self.interval.tick() => {
                    match self.sample().await {
                        Err(e) => {debug!(?e, "Failed to sample NTP source source.");},
                        Ok(ntp_event) => {
                            self.event_sender
                                .send(ntp_event.clone()).expect("Buffer Closing is not expected in alpha.");
                            debug!(?ntp_event, "Successfully sent NTP Source IO event.");
                        }
                    }
                }
                _ = self.ctrl_receiver.recv() => {
                    // Ctrl logic here.
                    // Currently we breakout of the loop if we receive a control event.
                    break;
                }
                _ = self.clock_disruption_receiver.changed() => {
                    // Clock Disruption logic here
                    todo!("Clock disruption logic has yet to be implemented.");
                }
            }
        }
        info!("NTP Source IO runner exiting.");
        Ok(())
    }
}
