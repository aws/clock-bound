//! Parsing and emitting NTP packets
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    reason = "TODO: Shamik please add documentation."
)]
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    reason = "TODO: Shamik please address various warning associated with casting."
)]
use std::fmt::Display;

use bon::Builder;
use nom::Parser;
use nom::number::{be_i8, be_u8};

mod extension;
mod header;
mod short;
mod timestamp;

pub use extension::ExtensionField;
pub use header::{LeapIndicator, Mode, Version};
pub use short::Short;
pub use timestamp::Timestamp;

use crate::daemon::event::{NtpData, Stratum, TryFromU8Error};
use crate::daemon::time::{Duration as ClockBoundDuration, Instant as ClockBoundInstant};

/// An NTP packet, as defined in RFC 5905
///
/// See each of the fields for more information
#[derive(Debug, Clone, PartialEq, Eq, Builder)]
pub struct Packet {
    /// warning of impending leap second
    #[builder(default = LeapIndicator::NoWarning)]
    pub leap_indicator: LeapIndicator,
    /// NTP version
    #[builder(default = Version::V4)]
    pub version: Version,
    /// Association mode
    #[builder(default = Mode::Client)]
    pub mode: Mode,
    /// Stratum
    #[builder(default = 0)]
    pub stratum: u8,
    /// Maximum interval between successive messages, in log2 seconds
    #[builder(default = 0)]
    pub poll: u8,
    /// Precision of system clock in log2 seconds
    #[builder(default = 0)]
    pub precision: i8,
    /// Total round trip delay to the reference clock
    #[builder(default = Short::new(0))]
    pub root_delay: Short,
    /// Total dispersion to the reference clock
    #[builder(default = Short::new(0))]
    pub root_dispersion: Short,
    /// 32 bit code identifying the particular server/reference clock
    #[builder(default = [0u8; 4])]
    pub reference_id: [u8; 4],
    /// Time when the system clock was last corrected
    #[builder(default = Timestamp::new(0))]
    pub reference_timestamp: Timestamp,
    /// Time at the client when the request departed for the server
    #[builder(default = Timestamp::new(0))]
    pub origin_timestamp: Timestamp,
    /// Time at the server when the request arrived from the client
    #[builder(default = Timestamp::new(0))]
    pub receive_timestamp: Timestamp,
    /// Time at the server when the response left for the client
    #[builder(default = Timestamp::new(0))]
    pub transmit_timestamp: Timestamp,
    /// Extension field(s), if any
    #[builder(default = Vec::new())]
    pub extensions: Vec<ExtensionField>,
}

impl Display for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "Packet")?;
        writeln!(f, "leap indicator: {:?}", self.leap_indicator)?;
        writeln!(f, "version: {:?}", self.version)?;
        writeln!(f, "mode: {:?}", self.mode)?;
        writeln!(f, "stratum: {}", self.stratum)?;
        writeln!(f, "poll: {}", self.poll)?;
        writeln!(f, "precision: {}", self.precision)?;
        writeln!(f, "root_delay: {}", self.root_delay.to_time_delta())?;
        writeln!(
            f,
            "root_dispersion: {}",
            self.root_dispersion.to_time_delta()
        )?;
        writeln!(f, "reference_id: {:?}", self.reference_id)?;
        writeln!(
            f,
            "reference_timestamp: {}",
            self.reference_timestamp.to_utc_datetime()
        )?;
        writeln!(
            f,
            "origin_timestamp: {}",
            self.origin_timestamp.to_utc_datetime()
        )?;
        writeln!(
            f,
            "receive_timestamp: {}",
            self.receive_timestamp.to_utc_datetime()
        )?;
        writeln!(
            f,
            "transmit_timestamp: {}",
            self.transmit_timestamp.to_utc_datetime()
        )?;
        Ok(())
    }
}

impl Packet {
    /// NTP packet size in bytes, without extensions
    pub const SIZE: usize = 48;

    /// Construct an NTP request packet using an arbitrary u32 as the transmit timestamp
    ///
    /// Convention is that the transmit timestamp does not need to correlate with any real timestamp
    /// in the NTP request. Hence, the `u64` input.
    ///
    /// If one wants to modify this constructor, they can simply do so via
    /// ```
    /// use clock_bound::daemon::io::ntp::packet::Packet;
    ///
    /// let modified_request = Packet {
    ///     poll: 4,
    ///     ..Packet::new_request(0xd00f)
    /// };
    /// ```
    pub const fn new_request(transmit_timestamp: u64) -> Self {
        let transmit_timestamp = Timestamp::new(transmit_timestamp);
        // These values don't HAVE to be zero, but keeping it simple for now.
        Self {
            leap_indicator: LeapIndicator::NoWarning,
            version: Version::V4,
            mode: Mode::Client,
            stratum: 0,
            poll: 0,
            precision: 0,
            root_delay: Short::new(0),
            root_dispersion: Short::new(0),
            reference_id: [0u8; 4],
            reference_timestamp: Timestamp::new(0),
            origin_timestamp: Timestamp::new(0),
            receive_timestamp: Timestamp::new(0),
            transmit_timestamp,
            extensions: Vec::new(),
        }
    }

    /// Emit packet into bytes
    ///
    /// ## Panics
    /// Panics if buffer is smaller than [`Packet::SIZE`]
    pub fn emit_bytes(&self, mut buffer: &mut [u8]) {
        use bytes::BufMut;
        let header: u8 =
            self.mode.to_bits() | self.version.get() << 3 | self.leap_indicator.to_bits() << 6;
        buffer.put_u8(header);
        buffer.put_u8(self.stratum);
        buffer.put_u8(self.poll);
        buffer.put_i8(self.precision);
        buffer.put_u32_ne(self.root_delay.into_network_endian());
        buffer.put_u32_ne(self.root_dispersion.into_network_endian());
        buffer.put(self.reference_id.as_slice());
        buffer.put_u64_ne(self.reference_timestamp.into_network_endian());
        buffer.put_u64_ne(self.origin_timestamp.into_network_endian());
        buffer.put_u64_ne(self.receive_timestamp.into_network_endian());
        buffer.put_u64_ne(self.transmit_timestamp.into_network_endian());
    }

    /// Parse from packet payload
    pub fn parse_from_bytes(input: &[u8]) -> nom::IResult<&[u8], Packet> {
        let (input, (leap_indicator, version, mode)) =
            nom::bits::bits((LeapIndicator::parse, Version::parse, Mode::parse)).parse(input)?;

        let (input, (stratum, poll, precision)) = (be_u8(), be_u8(), be_i8()).parse(input)?;

        let (input, (root_delay, root_dispersion)) = (Short::parse, Short::parse).parse(input)?;

        let (input, reference_id) = nom::bytes::take(4usize).parse(input)?;
        let reference_id: [u8; 4] = reference_id.try_into().unwrap();

        let (input, (reference_timestamp, origin_timestamp)) =
            (Timestamp::parse, Timestamp::parse).parse(input)?;
        let (input, (receive_timestamp, transmit_timestamp)) =
            (Timestamp::parse, Timestamp::parse).parse(input)?;

        let rv = Self {
            leap_indicator,
            version,
            mode,
            stratum,
            poll,
            precision,
            root_delay,
            root_dispersion,
            reference_id,
            reference_timestamp,
            origin_timestamp,
            receive_timestamp,
            transmit_timestamp,
            extensions: Vec::new(),
        };
        Ok((input, rv))
    }
}

impl TryFrom<Packet> for NtpData {
    type Error = TryFromU8Error;

    fn try_from(value: Packet) -> Result<Self, Self::Error> {
        let root_delay = ClockBoundDuration::from(value.root_delay);
        let root_dispersion = ClockBoundDuration::from(value.root_dispersion);
        let server_recv_time = ClockBoundInstant::from(value.receive_timestamp);
        let server_send_time = ClockBoundInstant::from(value.transmit_timestamp);

        let stratum = Stratum::try_from(value.stratum)?;

        Ok(NtpData {
            server_recv_time,
            server_send_time,

            root_delay,
            root_dispersion,

            stratum,
        })
    }
}

#[cfg(test)]
mod test {
    use chrono::{DateTime, TimeDelta};
    use hex_literal::hex;

    use crate::daemon::io::ntp::packet::extension::Fec2V1Value;

    use super::*;

    // test packet grabbed from wireshark
    // Network Time Protocol (NTP Version 4, server)
    // Flags: 0x24, Leap Indicator: no warning, Version number: NTP Version 4, Mode: server
    // [Request In: 5]
    // [Delta Time: 0.002007000 seconds]
    // Peer Clock Stratum: primary reference (1)
    // Peer Polling Interval: 3 (8 seconds)
    // Peer Clock Precision: -18 (0.000003815 seconds)
    // Root Delay: 0.000031 seconds
    // Root Dispersion: 0.000015 seconds
    // Reference ID: Unidentified reference source '���z'
    // Reference Timestamp: Jan 29, 2025 22:28:14.000004228 UTC // 1738189694 seconds
    // Origin Timestamp: Jan 29, 2025 22:28:14.687415882 UTC // 1738189694 seconds
    // Receive Timestamp: Jan 29, 2025 22:28:14.689357564 UTC // 1738189694 seconds
    // Transmit Timestamp: Jan 29, 2025 22:28:14.689379474 UTC // 1738189694 seconds
    const NTP_PACKET: &[u8] = &hex!(
        "
        240103ee0000000200000001a9fea97aeb4529fe
        000046f3eb4529feaffa7cbceb4529feb079bcc4
        eb4529feb07b2c5b
    "
    );

    #[test]
    fn parse() {
        let (leftover, packet) = Packet::parse_from_bytes(NTP_PACKET).unwrap();
        assert!(leftover.is_empty());
        assert_eq!(packet.leap_indicator, LeapIndicator::NoWarning);
        assert_eq!(packet.version.get(), 4);
        assert_eq!(packet.mode, Mode::Server);
        assert_eq!(packet.stratum, 1);
        assert_eq!(packet.poll, 3);
        assert_eq!(packet.precision, -18); // todo, create type which represents this better
        packet.root_delay.assert_within_error_bound(
            TimeDelta::from_std(std::time::Duration::from_secs_f64(0.000031)).unwrap(),
        );
        packet.root_dispersion.assert_within_error_bound(
            TimeDelta::from_std(std::time::Duration::from_secs_f64(0.000015)).unwrap(),
        );
        assert_eq!(packet.reference_id, [169, 254, 169, 122]);
        packet
            .reference_timestamp
            .assert_within_error_bound(DateTime::from_timestamp(1738189694, 000004228).unwrap());
        packet
            .origin_timestamp
            .assert_within_error_bound(DateTime::from_timestamp(1738189694, 687415882).unwrap());
        packet
            .receive_timestamp
            .assert_within_error_bound(DateTime::from_timestamp(1738189694, 689357564).unwrap());
        packet
            .transmit_timestamp
            .assert_within_error_bound(DateTime::from_timestamp(1738189694, 689379474).unwrap());
    }

    #[test]
    fn emit() {
        let (_, packet) = Packet::parse_from_bytes(NTP_PACKET).unwrap();
        let mut output = vec![0u8; Packet::SIZE];
        packet.emit_bytes(&mut output);
        assert_eq!(output, NTP_PACKET);
    }

    #[test]
    fn display_does_not_panic() {
        let packet = Packet::new_request(0xd00f_d00f);
        let _ = packet.to_string();
    }

    #[test]
    fn conversion_packet_to_ntp_data() {
        let (_, packet) = Packet::parse_from_bytes(NTP_PACKET).unwrap();
        let ntp_data = NtpData::try_from(packet).unwrap();

        assert!((ntp_data.root_delay.as_seconds_f64() - 0.000031).abs() < 1.0 / 65536.0);
        assert!((ntp_data.root_dispersion.as_seconds_f64() - 0.000015).abs() < 1.0 / 65536.0);
        assert_eq!(
            ntp_data.server_recv_time,
            ClockBoundInstant::from_time(1738189694, 689357564)
        );
        assert_eq!(
            ntp_data.server_send_time,
            ClockBoundInstant::from_time(1738189694, 689379474)
        );
        assert_eq!(ntp_data.stratum, Stratum::ONE);
    }

    #[test]
    fn packet_builder() {
        // Test with defaults
        let default_packet = Packet::builder().build();
        assert_eq!(default_packet.leap_indicator, LeapIndicator::NoWarning);
        assert_eq!(default_packet.version, Version::V4);
        assert_eq!(default_packet.mode, Mode::Client);
        assert_eq!(default_packet.stratum, 0);
        assert_eq!(default_packet.reference_id, [0u8; 4]);
        assert!(default_packet.extensions.is_empty());

        // Test with specified values
        let custom_packet = Packet::builder()
            .mode(Mode::Server)
            .stratum(1)
            .reference_id([0xa9, 0xfe, 0xa9, 0x7a])
            .extensions(vec![ExtensionField::Fec2V1(Fec2V1Value {
                major_version: 1,
                minor_version: 0,
                startup_id: 0xdeadbeef,
            })])
            .build();

        assert_eq!(custom_packet.mode, Mode::Server);
        assert_eq!(custom_packet.stratum, 1);
        assert_eq!(custom_packet.reference_id, [0xa9, 0xfe, 0xa9, 0x7a]);
        assert_eq!(custom_packet.extensions.len(), 1);
    }
}
