//! Thread-safe storage for the currently selected clock source

use std::{
    fmt::Display,
    net::Ipv4Addr,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::daemon::event::Stratum;

/// Thread-safe storage for the currently selected clock source and its stratum
///
/// Uses atomic operations to store both the clock source reference ID and stratum
/// in a single 64-bit value for lock-free access across threads.
#[derive(Debug)]
pub struct SelectedClockSource {
    /// Bits: 63-40 | 39-32  | 31-0
    ///       unused| stratum| refid
    source_info: AtomicU64,
}

impl SelectedClockSource {
    /// Get the current clock source and stratum
    ///
    /// Returns a tuple of (`ClockSource`, `Stratum`) representing the current state.
    pub fn get(&self) -> (ClockSource, Stratum) {
        let packed = self.source_info.load(Ordering::Relaxed);
        let refid = (packed & 0xFFFF_FFFF) as u32;
        let stratum =
            Stratum::try_from(((packed >> 32) & 0xFF) as u8).unwrap_or(Stratum::Unspecified);

        (Self::params_to_source(refid, stratum), stratum)
    }

    /// Set the clock source to PHC
    pub fn phc(&self) {
        self.set(ClockSource::Phc, Stratum::Unspecified);
    }

    /// Set the clock source to a remote NTP server
    pub fn server(&self, ip: Ipv4Addr, stratum: Stratum) {
        self.set(ClockSource::Server(ip), stratum);
    }

    /// Set the clock source to unsynchronized state
    pub fn none(&self) {
        self.set(ClockSource::None, Stratum::Unsynchronized);
    }

    /// Set the clock source to VMClock
    pub fn vmclock(&self) {
        self.set(ClockSource::VMClock, Stratum::Unspecified);
    }

    fn params_to_source(refid: u32, stratum: Stratum) -> ClockSource {
        match stratum {
            Stratum::Unspecified => {
                // Stratum 0 - interpret as kiss codes
                match refid {
                    v if v == u32::from_be_bytes(*b"INIT") => ClockSource::Init,
                    v if v == u32::from_be_bytes(*b"XPHC") => ClockSource::Phc,
                    v if v == u32::from_be_bytes(*b"XVMC") => ClockSource::VMClock,
                    _ => ClockSource::Init, // Defensively fallback to INIT;
                                            // Shouldn't occur given the restricted API
                }
            }
            Stratum::Unsynchronized => ClockSource::None,
            Stratum::Level(_) => ClockSource::Server(Ipv4Addr::from(refid)),
        }
    }

    fn set(&self, source: ClockSource, stratum: Stratum) {
        let packed = (u64::from(u8::from(stratum)) << 32) | u64::from(u32::from(source));
        self.source_info.store(packed, Ordering::Relaxed);
    }
}

impl Display for SelectedClockSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (source, stratum) = self.get();
        write!(f, "{} (stratum {})", source, u8::from(stratum))
    }
}

impl Default for SelectedClockSource {
    fn default() -> Self {
        let packed = (u64::from(u8::from(Stratum::Unspecified)) << 32)
            | u64::from(u32::from(ClockSource::Init));
        Self {
            source_info: AtomicU64::new(packed),
        }
    }
}

/// Clock source types
///
/// Represents different types of time sources that may be used for clock synchronization
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockSource {
    /// Initial state, never synchronized
    Init,
    /// Lost synchronization
    None,
    /// PTP Hardware Clock
    Phc,
    /// Remote NTP server
    Server(Ipv4Addr),
    /// Time and clock frequency from Linux hypervisor
    VMClock,
}

impl From<ClockSource> for u32 {
    fn from(source: ClockSource) -> u32 {
        match source {
            ClockSource::Init => u32::from_be_bytes(*b"INIT"),
            ClockSource::None => 0,
            ClockSource::Phc => u32::from_be_bytes(*b"XPHC"),
            ClockSource::Server(addr) => u32::from(addr),
            ClockSource::VMClock => u32::from_be_bytes(*b"XVMC"),
        }
    }
}

impl Display for ClockSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClockSource::Init => write!(f, "INIT"),
            ClockSource::None => write!(f, "None"),
            ClockSource::Phc => write!(f, "PHC"),
            ClockSource::Server(addr) => write!(f, "{addr}"),
            ClockSource::VMClock => write!(f, "VMClock"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[test]
    fn default_creates_init_state() {
        let clock = SelectedClockSource::default();
        let (source, stratum) = clock.get();

        assert_eq!(source, ClockSource::Init);
        assert_eq!(stratum, Stratum::Unspecified);
    }

    #[rstest]
    #[case(ClockSource::Init, Stratum::Unspecified)]
    #[case(ClockSource::Phc, Stratum::Unspecified)]
    #[case(ClockSource::VMClock, Stratum::Unspecified)]
    #[case(ClockSource::None, Stratum::Unsynchronized)]
    #[case(ClockSource::Server("192.168.1.1".parse().unwrap()), Stratum::TWO)]
    fn set_and_get_roundtrip(#[case] source: ClockSource, #[case] stratum: Stratum) {
        let clock = SelectedClockSource::default();
        clock.set(source.clone(), stratum);

        let (read_source, read_stratum) = clock.get();
        assert_eq!(read_source, source);
        assert_eq!(read_stratum, stratum);
    }

    #[test]
    fn convenience_methods() {
        let clock = SelectedClockSource::default();

        // Test PHC
        clock.phc();
        let (source, stratum) = clock.get();
        assert_eq!(source, ClockSource::Phc);
        assert_eq!(stratum, Stratum::Unspecified);

        // Test VMClock
        clock.vmclock();
        let (source, stratum) = clock.get();
        assert_eq!(source, ClockSource::VMClock);
        assert_eq!(stratum, Stratum::Unspecified);

        // Test Server
        let ip = "169.254.169.123".parse().unwrap();
        clock.server(ip, Stratum::ONE);
        let (source, stratum) = clock.get();
        assert_eq!(source, ClockSource::Server(ip));
        assert_eq!(stratum, Stratum::ONE);

        // Test Unsynchronized
        clock.none();
        let (source, stratum) = clock.get();
        assert_eq!(source, ClockSource::None);
        assert_eq!(stratum, Stratum::Unsynchronized);
    }

    #[rstest]
    #[case(ClockSource::Init, "INIT")]
    #[case(ClockSource::None, "None")]
    #[case(ClockSource::Phc, "PHC")]
    #[case(ClockSource::VMClock, "VMClock")]
    #[case(ClockSource::Server("192.168.1.1".parse().unwrap()), "192.168.1.1")]
    fn clock_source_display(#[case] source: ClockSource, #[case] expected: &str) {
        assert_eq!(source.to_string(), expected);
    }

    #[rstest]
    #[case(ClockSource::Init, 0x494E_4954)] // "INIT"
    #[case(ClockSource::None, 0)]
    #[case(ClockSource::Phc, 0x5850_4843)] // "XPHC"
    #[case(ClockSource::VMClock, 0x5856_4D43)] // "XVMC"
    #[case(ClockSource::Server("192.168.1.1".parse().unwrap()), 0xC0A8_0101)]
    fn clock_source_to_u32(#[case] source: ClockSource, #[case] expected: u32) {
        assert_eq!(u32::from(source), expected);
    }

    #[test]
    fn selected_clock_source_display() {
        let clock = SelectedClockSource::default();
        assert_eq!(clock.to_string(), "INIT (stratum 0)");

        clock.phc();
        assert_eq!(clock.to_string(), "PHC (stratum 0)");

        clock.server("169.254.169.123".parse().unwrap(), Stratum::ONE);
        assert_eq!(clock.to_string(), "169.254.169.123 (stratum 1)");

        clock.none();
        assert_eq!(clock.to_string(), "None (stratum 16)");
    }
}
