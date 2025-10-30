//! Link Local IO Source

use thiserror::Error;
use tokio::{
    io,
    net::UdpSocket,
    sync::{mpsc, watch},
    time::{self, Interval, MissedTickBehavior, interval, timeout},
};
use tracing::{debug, info};

use super::tsc::read_timestamp_counter;
use super::{ClockDisruptionEvent, ControlRequest};
use crate::daemon::{
    async_ring_buffer,
    event::{self, NtpData},
    time::tsc::TscCount,
};

use super::ntp::{LINK_LOCAL_ADDRESS, LINK_LOCAL_INTERVAL_DURATION, LINK_LOCAL_TIMEOUT, packet};
use packet::Packet;

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
    event_sender: async_ring_buffer::Sender<event::Ntp>,
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    ntp_buffer: [u8; Packet::SIZE],
    interval: Interval,
}

impl LinkLocal {
    /// Constructs a new `LinkLocal` with using given parameters.
    pub fn construct(
        socket: UdpSocket,
        event_sender: async_ring_buffer::Sender<event::Ntp>,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    ) -> Self {
        let mut link_local_interval = interval(LINK_LOCAL_INTERVAL_DURATION);
        link_local_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        LinkLocal {
            socket,
            event_sender,
            ctrl_receiver,
            clock_disruption_receiver,
            ntp_buffer: [0u8; Packet::SIZE],
            interval: link_local_interval,
        }
    }

    /// Samples the Link local source.
    ///
    /// When sampling from a NTP source we first collect the current time stamp counter
    /// value. We then send a NTP request and await for a response. Once we receive a
    /// response we again collect the current time stamp counter value. After we've
    /// collected the NTP sample we construct the `Event` and push that event through
    /// to the ring buffer.
    async fn sample(&mut self) -> Result<event::Ntp, LinkLocalError> {
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
        Ok(ntp_event)
    }

    /// NTP Link Local task runner.
    ///
    /// Sampling NTP packets from the AWS EC2 internal Link Local address.
    ///
    /// # Panics
    /// Function will panic if not called within the `tokio` runtime.
    ///
    /// # Errors
    /// Returns error if loop exits unexpectedly
    pub async fn run(&mut self) -> Result<(), LinkLocalError> {
        // Sampling loop
        info!("Starting link local sampling loop.");
        loop {
            tokio::select! {
                _ = self.interval.tick() => {
                    match self.sample().await {
                        Err(e) => {debug!(?e, "Failed to sample link local source.");}
                        Ok(ntp_event) => {
                            self.event_sender.send(ntp_event.clone()).expect("Buffer Closing is not expected in alpha.");
                            debug!(?ntp_event, "Successfully sent Link Local IO event.");
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
        info!("Link local runner exiting.");
        Ok(())
    }
}
