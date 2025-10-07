//! Header components in the NTP packet

use nom::Parser;

/// Leap indicator of an NTP packet
///
/// See [RFC 5905](https://datatracker.ietf.org/doc/html/rfc5905#section-7.3)
/// for more information
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LeapIndicator {
    /// No Warning
    NoWarning,
    /// Last minute of the day had 61 seconds
    LastMinute61Seconds,
    /// Last minute of the day had 59 seconds
    LastMinute59Seconds,
    /// Unknown (Clock unsynchronized)
    Unknown,
}

impl LeapIndicator {
    /// parse from bits
    pub fn parse(input: Bits) -> nom::IResult<Bits, Self> {
        // unwrap ok. max value of 2 bits is 3
        nom::bits::complete::take(2usize)
            .map(|val| Self::from_bits(val).unwrap())
            .parse(input)
    }

    /// Construct from network bits
    ///
    /// Returns none is val >= 4
    pub fn from_bits(val: u8) -> Option<Self> {
        match val {
            0 => Some(Self::NoWarning),
            1 => Some(Self::LastMinute61Seconds),
            2 => Some(Self::LastMinute59Seconds),
            3 => Some(Self::Unknown),
            _ => None,
        }
    }

    /// Construct network bits
    pub fn to_bits(self) -> u8 {
        match self {
            Self::NoWarning => 0,
            Self::LastMinute61Seconds => 1,
            Self::LastMinute59Seconds => 2,
            Self::Unknown => 3,
        }
    }
}

/// Mode of an NTP packet
///
/// See [RFC 5905](https://datatracker.ietf.org/doc/html/rfc5905#section-7.3)
/// for more information
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    /// reserved
    Reserved,
    /// symmetric active
    SymmetricActive,
    /// symmetric passive
    SymmetricPassive,
    /// client
    Client,
    /// server
    Server,
    /// broadcast
    Broadcast,
    /// ntp control message
    NtpControlMessage,
    /// reserved for private use
    ReservedForPrivateUse,
}

impl Mode {
    /// parse from bits
    pub fn parse(input: Bits) -> nom::IResult<Bits, Self> {
        nom::bits::complete::take(3usize)
            .map(|val| Self::from_bits(val).unwrap())
            .parse(input)
    }

    /// Construct from network bits
    pub fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Reserved),
            1 => Some(Self::SymmetricActive),
            2 => Some(Self::SymmetricPassive),
            3 => Some(Self::Client),
            4 => Some(Self::Server),
            5 => Some(Self::Broadcast),
            6 => Some(Self::NtpControlMessage),
            7 => Some(Self::ReservedForPrivateUse),
            _ => None,
        }
    }

    /// Construct network bits
    pub fn to_bits(self) -> u8 {
        match self {
            Self::Reserved => 0,
            Self::SymmetricActive => 1,
            Self::SymmetricPassive => 2,
            Self::Client => 3,
            Self::Server => 4,
            Self::Broadcast => 5,
            Self::NtpControlMessage => 6,
            Self::ReservedForPrivateUse => 7,
        }
    }
}

/// The 3-bit version field in the NTP header
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    inner: u8,
}

impl Version {
    /// Version 4
    pub const V4: Self = Self { inner: 4 };

    /// parse from bits
    pub fn parse(input: Bits) -> nom::IResult<Bits, Self> {
        nom::bits::complete::take(3usize)
            .map(|val| Self::from_bits(val).unwrap())
            .parse(input)
    }

    /// Constructor
    ///
    /// Returns Some if value is 3 bits or less
    pub const fn from_bits(value: u8) -> Option<Self> {
        if value <= 0b111 {
            Some(Self { inner: value })
        } else {
            None
        }
    }

    /// Get version as u8
    pub const fn get(self) -> u8 {
        self.inner
    }
}

type Bits<'a> = (&'a [u8], usize);

#[cfg(test)]
mod test {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(&[0 << 6], LeapIndicator::NoWarning)]
    #[case(&[1 << 6], LeapIndicator::LastMinute61Seconds)]
    #[case(&[2 << 6], LeapIndicator::LastMinute59Seconds)]
    #[case(&[3 << 6], LeapIndicator::Unknown)]
    fn parse_leap_indicator(#[case] byte: &[u8], #[case] expected: LeapIndicator) {
        let res: nom::IResult<&[u8], LeapIndicator> =
            nom::bits::bits(LeapIndicator::parse).parse(byte);
        let res = res.unwrap();
        assert_eq!(res.1, expected);
        assert!(res.0.is_empty());
    }

    #[rstest]
    #[case(0, LeapIndicator::NoWarning)]
    #[case(1, LeapIndicator::LastMinute61Seconds)]
    #[case(2, LeapIndicator::LastMinute59Seconds)]
    #[case(3, LeapIndicator::Unknown)]
    fn leap_indicator_to_bits(#[case] expected: u8, #[case] leap_indicator: LeapIndicator) {
        let bits = leap_indicator.to_bits();
        assert_eq!(bits, expected);
    }

    #[rstest]
    #[case(0)]
    #[case(1)]
    #[case(7)]
    #[case(4)]
    fn parse_version(#[case] byte: u8) {
        let input = [byte << 5];
        let input = input.as_slice();
        let res: nom::IResult<&[u8], Version> = nom::bits::bits(Version::parse).parse(input);
        let res = res.unwrap();
        assert_eq!(res.1, Version::from_bits(byte).unwrap());
        assert!(res.0.is_empty());
    }

    #[rstest]
    #[case(&[0 << 5], Mode::Reserved)]
    #[case(&[1 << 5], Mode::SymmetricActive)]
    #[case(&[2 << 5], Mode::SymmetricPassive)]
    #[case(&[3 << 5], Mode::Client)]
    #[case(&[4 << 5], Mode::Server)]
    #[case(&[5 << 5], Mode::Broadcast)]
    #[case(&[6 << 5], Mode::NtpControlMessage)]
    #[case(&[7 << 5], Mode::ReservedForPrivateUse)]
    fn parse_mode(#[case] byte: &[u8], #[case] expected: Mode) {
        let res: nom::IResult<&[u8], Mode> = nom::bits::bits(Mode::parse).parse(byte);
        let res = res.unwrap();
        assert_eq!(res.1, expected);
        assert!(res.0.is_empty());
    }

    #[rstest]
    #[case(0, Mode::Reserved)]
    #[case(1, Mode::SymmetricActive)]
    #[case(2, Mode::SymmetricPassive)]
    #[case(3, Mode::Client)]
    #[case(4, Mode::Server)]
    #[case(5, Mode::Broadcast)]
    #[case(6, Mode::NtpControlMessage)]
    #[case(7, Mode::ReservedForPrivateUse)]
    fn mode_to_bits(#[case] expected: u8, #[case] mode: Mode) {
        let bits = mode.to_bits();
        assert_eq!(bits, expected);
    }
}
