//! NTP Time synchronization events
use std::{
    error::Error,
    fmt::{Display, Formatter},
};

use super::TscRtt;
use crate::daemon::time::{Duration, Instant, TscCount};

/// Contains the NTP and time stamp counter samples to be used by synchronization algorithm.
///
/// `tsc_post` must be greater than `tsc_pre`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ntp {
    /// TSC value before sending event
    tsc_pre: TscCount,
    /// TSC value after sending event
    tsc_post: TscCount,
    /// NTP Packet data
    data: NtpData,
}

#[bon::bon]
impl Ntp {
    /// Construct a [`Ntp`]
    ///
    /// Returns `None` if `tsc_post <= tsc_pre`
    #[builder]
    pub fn new(tsc_pre: TscCount, tsc_post: TscCount, ntp_data: NtpData) -> Option<Self> {
        if tsc_post > tsc_pre {
            Some(Self {
                tsc_pre,
                tsc_post,
                data: ntp_data,
            })
        } else {
            None
        }
    }
}

impl Ntp {
    /// `tsc_pre` getter
    pub fn tsc_pre(&self) -> TscCount {
        self.tsc_pre
    }

    /// `tsc_post` getter
    pub fn tsc_post(&self) -> TscCount {
        self.tsc_post
    }

    /// `data` getter
    pub fn data(&self) -> &NtpData {
        &self.data
    }
}

impl TscRtt for Ntp {
    fn tsc_pre(&self) -> TscCount {
        self.tsc_pre
    }

    fn tsc_post(&self) -> TscCount {
        self.tsc_post
    }
}

/// NTP-specific data
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NtpData {
    /// NTP Server recv time
    pub server_recv_time: Instant,
    /// NTP Server send time
    pub server_send_time: Instant,

    /// Root Delay of NTP packet
    pub root_delay: Duration,
    /// Root Dispersion of NTP packet
    pub root_dispersion: Duration,

    /// NTP Stratum. Used in reporting, not used in ff-sync
    pub stratum: Stratum,
}

/// An NTP stratum
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stratum {
    /// Unspecified or invalid.
    ///
    /// Corresponds to a value of 0 in an NTP packet
    Unspecified,
    /// A server stratum level
    ///
    /// Corresponds to a value of 1-15 in an NTP packet
    Level(ValidStratumLevel),
    /// Clock is unsynchronized
    ///
    /// Corresponds to a value of 16 in an NTP packet
    Unsynchronized,
}

impl Stratum {
    /// Stratum 1
    pub const ONE: Self = Self::Level(ValidStratumLevel(1));

    /// Stratum 2
    pub const TWO: Self = Self::Level(ValidStratumLevel(2));

    /// Construct a new stratum from a `u8` value
    ///
    /// Returns none if the value is > 16
    pub const fn new(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Unspecified),
            16 => Some(Self::Unsynchronized),
            1..=15 => match ValidStratumLevel::new(value) {
                Some(level) => Some(Self::Level(level)),
                None => None,
            },
            _ => None,
        }
    }
}

impl From<Stratum> for u8 {
    fn from(stratum: Stratum) -> Self {
        match stratum {
            Stratum::Unspecified => 0,
            Stratum::Level(level) => level.get(),
            Stratum::Unsynchronized => 16,
        }
    }
}

impl TryFrom<u8> for Stratum {
    type Error = TryFromU8Error;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Stratum::new(value).ok_or(TryFromU8Error)
    }
}

/// The error type returned when a checked integral type conversion fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TryFromU8Error;

impl Display for TryFromU8Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("invalid value")
    }
}

impl Error for TryFromU8Error {}

/// A valid stratum level, from 1 to 15
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ValidStratumLevel(u8);

impl ValidStratumLevel {
    /// Create a new valid stratum level
    /// Returns None if the value is not between 1 and 15
    pub const fn new(value: u8) -> Option<Self> {
        if value > 0 && value <= 15 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Get the inner value
    pub fn get(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ntp_event() {
        let event = Ntp::builder()
            .tsc_pre(TscCount::new(1))
            .tsc_post(TscCount::new(2))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1),
                server_send_time: Instant::new(2),
                root_delay: Duration::new(3),
                root_dispersion: Duration::new(4),
                stratum: Stratum::ONE,
            })
            .build();

        let event = event.unwrap();

        assert_eq!(event.tsc_pre().get(), 1);
        assert_eq!(event.tsc_post().get(), 2);
        assert_eq!(event.data().server_recv_time, Instant::new(1));
        assert_eq!(event.data().server_send_time, Instant::new(2));
        assert_eq!(event.data().root_delay, Duration::new(3));
        assert_eq!(event.data().root_dispersion, Duration::new(4));
        assert_eq!(event.data().stratum, Stratum::ONE);
    }

    #[test]
    fn wrong_tsc_order() {
        let event = Ntp::builder()
            .tsc_pre(TscCount::new(2))
            .tsc_post(TscCount::new(1))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1),
                server_send_time: Instant::new(2),
                root_delay: Duration::new(3),
                root_dispersion: Duration::new(4),
                stratum: Stratum::ONE,
            })
            .build();

        assert!(event.is_none());
    }

    #[test]
    fn stratum_new_valid_values() {
        assert_eq!(Stratum::new(0), Some(Stratum::Unspecified));
        assert_eq!(Stratum::new(16), Some(Stratum::Unsynchronized));

        // Test valid levels 1-15
        for i in 1..=15 {
            let stratum = Stratum::new(i);
            assert!(stratum.is_some());
            let Some(Stratum::Level(level)) = stratum else {
                panic!("Expected Stratum::Level for value {}", i);
            };
            assert_eq!(level.get(), i);
        }
    }

    #[test]
    fn stratum_new_invalid_values() {
        assert_eq!(Stratum::new(17), None);
        assert_eq!(Stratum::new(255), None);
    }

    #[test]
    fn stratum_conversion_to_u8() {
        assert_eq!(u8::from(Stratum::Unspecified), 0);
        assert_eq!(u8::from(Stratum::Unsynchronized), 16);

        // Test conversion of valid levels
        for i in 1..=15 {
            let level = ValidStratumLevel::new(i).unwrap();
            assert_eq!(u8::from(Stratum::Level(level)), i);
        }
    }

    #[test]
    fn stratum_try_from_u8() {
        // Test valid conversions
        assert!(matches!(Stratum::try_from(0), Ok(Stratum::Unspecified)));
        assert!(matches!(Stratum::try_from(16), Ok(Stratum::Unsynchronized)));

        // Test valid levels
        for i in 1..=15 {
            let result = Stratum::try_from(i);
            assert!(result.is_ok());
            assert!(matches!(result.unwrap(), Stratum::Level(_)));
        }
    }

    #[test]
    fn invalid_try_from_u8() {
        // Test invalid conversions
        assert!(matches!(Stratum::try_from(17), Err(TryFromU8Error)));
        assert!(matches!(Stratum::try_from(255), Err(TryFromU8Error)));
    }
}
