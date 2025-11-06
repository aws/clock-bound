//! Thread-safe storage for the currently selected clock source

use std::{
    fmt::Display,
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
};

use md5;

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
    pub fn set_to_phc(&self) {
        self.set(ClockSource::Phc, Stratum::Unspecified);
    }

    /// Set the clock source to a remote NTP server
    pub fn set_to_server(&self, ip: IpAddr, stratum: Stratum) {
        let refid = match ip {
            IpAddr::V4(ipv4) => u32::from(ipv4),
            IpAddr::V6(ipv6) => {
                let hash = md5::compute(ipv6.octets());
                u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]])
            }
        };
        self.set(ClockSource::Server(refid), stratum);
    }

    /// Set the clock source to unsynchronized state
    pub fn set_to_none(&self) {
        self.set(ClockSource::None, Stratum::Unsynchronized);
    }

    /// Set the clock source to VMClock
    pub fn set_to_vmclock(&self) {
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
                    _ => {
                        let bytes = refid.to_be_bytes();
                        unreachable!(
                            "Unknown kiss code [{}, {}, {}, {}]; should not occur with restricted API",
                            bytes[0], bytes[1], bytes[2], bytes[3]
                        )
                    }
                }
            }
            Stratum::Level(_) => ClockSource::Server(refid),
            Stratum::Unsynchronized => ClockSource::None,
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
    /// NTP server (stores reference ID: IPv4 address or first 4 octets of IPv6 MD5 hash)
    Server(u32),
    /// Time and clock frequency from Linux hypervisor
    VMClock,
}

impl From<ClockSource> for u32 {
    fn from(source: ClockSource) -> u32 {
        match source {
            ClockSource::Init => u32::from_be_bytes(*b"INIT"),
            ClockSource::None => 0,
            ClockSource::Phc => u32::from_be_bytes(*b"XPHC"),
            ClockSource::Server(refid) => refid,
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
            ClockSource::Server(refid) => {
                let bytes = refid.to_be_bytes();
                write!(
                    f,
                    "Server([{}, {}, {}, {}])",
                    bytes[0], bytes[1], bytes[2], bytes[3]
                )
            }
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
    #[case(ClockSource::Server(0xC0A80101), Stratum::TWO)] // 192.168.1.1
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
        clock.set_to_phc();
        let (source, stratum) = clock.get();
        assert_eq!(source, ClockSource::Phc);
        assert_eq!(stratum, Stratum::Unspecified);

        // Test VMClock
        clock.set_to_vmclock();
        let (source, stratum) = clock.get();
        assert_eq!(source, ClockSource::VMClock);
        assert_eq!(stratum, Stratum::Unspecified);

        // Test Server IPv4
        let ip: IpAddr = "169.254.169.123".parse().unwrap();
        clock.set_to_server(ip, Stratum::ONE);
        let (source, stratum) = clock.get();
        assert_eq!(source, ClockSource::Server(0xA9FEA97B)); // 169.254.169.123 as u32
        assert_eq!(stratum, Stratum::ONE);

        // Test Server IPv6
        let ipv6: IpAddr = "2001:db8::1".parse().unwrap();
        clock.set_to_server(ipv6, Stratum::TWO);
        let (source, stratum) = clock.get();
        // MD5 hash of 2001:db8::1 is 39ab9b3749629b8f2c7ccf39226f680c
        // First 4 octets: 39ab9b37
        assert_eq!(source, ClockSource::Server(0x39ab9b37));
        assert_eq!(stratum, Stratum::TWO);

        // Test Unsynchronized
        clock.set_to_none();
        let (source, stratum) = clock.get();
        assert_eq!(source, ClockSource::None);
        assert_eq!(stratum, Stratum::Unsynchronized);
    }

    #[rstest]
    #[case(ClockSource::Init, "INIT")]
    #[case(ClockSource::None, "None")]
    #[case(ClockSource::Phc, "PHC")]
    #[case(ClockSource::VMClock, "VMClock")]
    #[case(ClockSource::Server(0xC0A80101), "Server([192, 168, 1, 1])")]
    fn clock_source_display(#[case] source: ClockSource, #[case] expected: &str) {
        assert_eq!(source.to_string(), expected);
    }

    #[rstest]
    #[case(ClockSource::Init, 0x494E_4954)] // "INIT"
    #[case(ClockSource::None, 0)]
    #[case(ClockSource::Phc, 0x5850_4843)] // "XPHC"
    #[case(ClockSource::VMClock, 0x5856_4D43)] // "XVMC"
    #[case(ClockSource::Server(0xC0A8_0101), 0xC0A8_0101)]
    fn clock_source_to_u32(#[case] source: ClockSource, #[case] expected: u32) {
        assert_eq!(u32::from(source), expected);
    }

    #[test]
    fn selected_clock_source_display() {
        let clock = SelectedClockSource::default();
        assert_eq!(clock.to_string(), "INIT (stratum 0)");

        clock.set_to_phc();
        assert_eq!(clock.to_string(), "PHC (stratum 0)");

        clock.set_to_vmclock();
        assert_eq!(clock.to_string(), "VMClock (stratum 0)");

        clock.set_to_server("169.254.169.123".parse().unwrap(), Stratum::ONE);
        assert_eq!(
            clock.to_string(),
            "Server([169, 254, 169, 123]) (stratum 1)"
        );

        clock.set_to_server("169.254.169.123".parse().unwrap(), Stratum::TWO);
        assert_eq!(
            clock.to_string(),
            "Server([169, 254, 169, 123]) (stratum 2)"
        );

        clock.set_to_none();
        assert_eq!(clock.to_string(), "None (stratum 16)");
    }
}
