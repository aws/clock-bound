//! Log-owned schema types for the feed-forward events (`ffevents.log`) repro log.

use std::fmt::{self, Display, Formatter};
use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::event::{self, NtpData, PhcData, Stratum};
use crate::daemon::receiver_stream::RoutableEvent;
use crate::daemon::time::{Duration, Instant, TscCount, tsc::Period};

/// TODO: FIXME!
/// Temporarily hardcoded default PHC device path used as the serialized `identifier` for PHC events.
///
/// The actual PHC device path is autoconfigured at runtime and may differ from this
/// value on instances with multiple ENA interfaces or non-standard PTP device enumeration.
/// A future change should thread the real device path through to this serialization point.
const PHC_IDENTIFIER: &str = "/dev/ptp0";

// TODO: FIXME!
// Temporarily hardcoded link-local address used as the serialized `identifier` for `AmazonTimeSync` events.
// Currently hardcoded because the AmazonTimeSync endpoint is always at this fixed link-local address.
//
// This will not hold for non-EC2 environments that happen to have an NTP server at the same
// address. A future change should thread an Amazon-specific identifier field into the serialized
// data/logs.
const AMAZON_TIME_SYNC_ADDRESS_STR: &str = "169.254.169.123:123";

/// Log schema for a [`RoutableEvent`] as written to `ffevents.log`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum RoutableEventLog {
    Ntp {
        identifier: String,
        counter_pre: TscCount,
        counter_post: TscCount,
        data: NtpDataLog,
    },
    Phc {
        identifier: String,
        counter_pre: TscCount,
        counter_post: TscCount,
        data: PhcDataLog,
    },
}

/// Log schema for [`NtpData`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NtpDataLog {
    server_recv_time_ns: Instant,
    server_send_time_ns: Instant,
    root_delay_ns: Duration,
    root_dispersion_ns: Duration,
    stratum: Stratum,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PhcDataLog {
    time_ns: Instant,
    clock_error_bound_ns: Duration,
}

/// Log schema for [`ClockParameters`] output.
///
/// Note that `as_of_monotonic` is intentionally absent from the log schema.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClockParametersLog {
    counter_sync: TscCount,
    time_sync_ns: Instant,
    clock_error_bound_ns: Duration,
    period_s: Period,
    period_max_error_s: Period,
}

/// Error produced when converting a [`RoutableEventLog`] back into a
/// [`RoutableEvent`].
#[derive(Debug)]
pub enum RoutableEventLogError {
    /// The NTP builder rejected the counters (`counter_post <= counter_pre`).
    InvalidNtpCounters,
    /// The PHC builder rejected the counters (`counter_post <= counter_pre`).
    InvalidPhcCounters,
    /// The `identifier` field was not a valid socket address.
    InvalidSocketAddr(std::net::AddrParseError),
}

impl Display for RoutableEventLogError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNtpCounters => {
                f.write_str("invalid NTP event: counter_post must be greater than counter_pre")
            }
            Self::InvalidPhcCounters => {
                f.write_str("invalid PHC event: counter_post must be greater than counter_pre")
            }
            Self::InvalidSocketAddr(e) => write!(f, "invalid socket address: {e}"),
        }
    }
}

impl std::error::Error for RoutableEventLogError {}

// --- NtpData <-> NtpDataLog ---

impl From<&NtpData> for NtpDataLog {
    fn from(data: &NtpData) -> Self {
        let NtpData {
            server_recv_time,
            server_send_time,
            root_delay,
            root_dispersion,
            stratum,
        } = data;
        Self {
            server_recv_time_ns: *server_recv_time,
            server_send_time_ns: *server_send_time,
            root_delay_ns: *root_delay,
            root_dispersion_ns: *root_dispersion,
            stratum: *stratum,
        }
    }
}

impl From<NtpDataLog> for NtpData {
    fn from(log: NtpDataLog) -> Self {
        let NtpDataLog {
            server_recv_time_ns,
            server_send_time_ns,
            root_delay_ns,
            root_dispersion_ns,
            stratum,
        } = log;
        Self {
            server_recv_time: server_recv_time_ns,
            server_send_time: server_send_time_ns,
            root_delay: root_delay_ns,
            root_dispersion: root_dispersion_ns,
            stratum,
        }
    }
}

// --- PhcData <-> PhcDataLog ---

impl From<&PhcData> for PhcDataLog {
    fn from(data: &PhcData) -> Self {
        let PhcData {
            time,
            clock_error_bound,
        } = data;
        Self {
            time_ns: *time,
            clock_error_bound_ns: *clock_error_bound,
        }
    }
}

impl From<PhcDataLog> for PhcData {
    fn from(log: PhcDataLog) -> Self {
        let PhcDataLog {
            time_ns,
            clock_error_bound_ns,
        } = log;
        Self {
            time: time_ns,
            clock_error_bound: clock_error_bound_ns,
        }
    }
}

// --- RoutableEvent <-> RoutableEventLog ---

impl From<&RoutableEvent> for RoutableEventLog {
    fn from(event: &RoutableEvent) -> Self {
        match event {
            RoutableEvent::AmazonTimeSync(ntp) => Self::Ntp {
                identifier: AMAZON_TIME_SYNC_ADDRESS_STR.to_string(),
                counter_pre: ntp.counter_pre(),
                counter_post: ntp.counter_post(),
                data: ntp.data().into(),
            },
            RoutableEvent::NtpSource(addr, ntp) => Self::Ntp {
                identifier: addr.to_string(),
                counter_pre: ntp.counter_pre(),
                counter_post: ntp.counter_post(),
                data: ntp.data().into(),
            },
            RoutableEvent::Phc(phc) => Self::Phc {
                identifier: PHC_IDENTIFIER.to_string(),
                counter_pre: phc.counter_pre(),
                counter_post: phc.counter_post(),
                data: phc.data().into(),
            },
        }
    }
}

impl TryFrom<RoutableEventLog> for RoutableEvent {
    type Error = RoutableEventLogError;

    fn try_from(log: RoutableEventLog) -> Result<Self, Self::Error> {
        match log {
            RoutableEventLog::Ntp {
                identifier,
                counter_pre,
                counter_post,
                data,
            } => {
                let ntp = event::Ntp::builder()
                    .counter_pre(counter_pre)
                    .counter_post(counter_post)
                    .ntp_data(data.into())
                    .build()
                    .ok_or(RoutableEventLogError::InvalidNtpCounters)?;

                if identifier == AMAZON_TIME_SYNC_ADDRESS_STR {
                    Ok(RoutableEvent::AmazonTimeSync(ntp))
                } else {
                    let addr: SocketAddr = identifier
                        .parse()
                        .map_err(RoutableEventLogError::InvalidSocketAddr)?;
                    Ok(RoutableEvent::NtpSource(addr, ntp))
                }
            }
            RoutableEventLog::Phc {
                identifier: _,
                counter_pre,
                counter_post,
                data,
            } => {
                let phc = event::Phc::builder()
                    .counter_pre(counter_pre)
                    .counter_post(counter_post)
                    .data(data.into())
                    .build()
                    .ok_or(RoutableEventLogError::InvalidPhcCounters)?;

                Ok(RoutableEvent::Phc(phc))
            }
        }
    }
}

// --- ClockParameters <-> ClockParametersLog ---

impl From<&ClockParameters> for ClockParametersLog {
    fn from(params: &ClockParameters) -> Self {
        let ClockParameters {
            tsc_count,
            time,
            clock_error_bound,
            period,
            period_max_error,
            as_of_monotonic: _,
        } = params;
        Self {
            counter_sync: *tsc_count,
            time_sync_ns: *time,
            clock_error_bound_ns: *clock_error_bound,
            period_s: *period,
            period_max_error_s: *period_max_error,
        }
    }
}

impl From<ClockParametersLog> for ClockParameters {
    fn from(log: ClockParametersLog) -> Self {
        let ClockParametersLog {
            counter_sync,
            time_sync_ns,
            clock_error_bound_ns,
            period_s,
            period_max_error_s: period_max_error,
        } = log;
        Self {
            tsc_count: counter_sync,
            time: time_sync_ns,
            clock_error_bound: clock_error_bound_ns,
            period: period_s,
            period_max_error,
            // Reconstruct the same default the old `#[serde(skip)]` produced.
            as_of_monotonic: Instant::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::event::{Ntp, NtpData, Phc, PhcData, Stratum};
    use crate::daemon::time::{Duration, Instant, TscCount};

    // --- RoutableEvent log serialization tests (moved from receiver_stream.rs) ---

    #[test]
    fn serialize_ntp_amazon_time_sync() {
        let event = Ntp::builder()
            .counter_pre(TscCount::new(100))
            .counter_post(TscCount::new(200))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1000),
                server_send_time: Instant::new(2000),
                root_delay: Duration::new(50),
                root_dispersion: Duration::new(25),
                stratum: Stratum::ONE,
            })
            .build()
            .unwrap();

        let routable = RoutableEvent::AmazonTimeSync(event);
        let actual: serde_json::Value =
            serde_json::to_value(RoutableEventLog::from(&routable)).unwrap();
        let expected: serde_json::Value = serde_json::from_str(
            r#"
            {
                "type": "ntp",
                "identifier": "169.254.169.123:123",
                "counter_pre": 100,
                "counter_post": 200,
                "data": {
                    "server_recv_time_ns": 1000,
                    "server_send_time_ns": 2000,
                    "root_delay_ns": 50,
                    "root_dispersion_ns": 25,
                    "stratum": 1
                }
            }
        "#,
        )
        .unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn serialize_ntp_source() {
        let event = Ntp::builder()
            .counter_pre(TscCount::new(100))
            .counter_post(TscCount::new(200))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1000),
                server_send_time: Instant::new(2000),
                root_delay: Duration::new(50),
                root_dispersion: Duration::new(25),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        let addr: SocketAddr = "10.0.0.1:123".parse().unwrap();
        let routable = RoutableEvent::NtpSource(addr, event);
        let actual: serde_json::Value =
            serde_json::to_value(RoutableEventLog::from(&routable)).unwrap();
        let expected: serde_json::Value = serde_json::from_str(
            r#"
            {
                "type": "ntp",
                "identifier": "10.0.0.1:123",
                "counter_pre": 100,
                "counter_post": 200,
                "data": {
                    "server_recv_time_ns": 1000,
                    "server_send_time_ns": 2000,
                    "root_delay_ns": 50,
                    "root_dispersion_ns": 25,
                    "stratum": 2
                }
            }
        "#,
        )
        .unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn serialize_phc() {
        let event = Phc::builder()
            .counter_pre(TscCount::new(300))
            .counter_post(TscCount::new(400))
            .data(PhcData {
                time: Instant::new(5000),
                clock_error_bound: Duration::new(100),
            })
            .build()
            .unwrap();

        let routable = RoutableEvent::Phc(event);
        let actual: serde_json::Value =
            serde_json::to_value(RoutableEventLog::from(&routable)).unwrap();
        let expected: serde_json::Value = serde_json::from_str(
            r#"
            {
                "type": "phc",
                "identifier": "/dev/ptp0",
                "counter_pre": 300,
                "counter_post": 400,
                "data": {
                    "time_ns": 5000,
                    "clock_error_bound_ns": 100
                }
            }
        "#,
        )
        .unwrap();

        assert_eq!(actual, expected);
    }

    // --- RoutableEvent log round-trip tests (moved from receiver_stream.rs) ---

    /// Round-trip a domain event through the log schema and back.
    fn round_trip(original: &RoutableEvent) -> RoutableEvent {
        let json = serde_json::to_string(&RoutableEventLog::from(original)).unwrap();
        let log: RoutableEventLog = serde_json::from_str(&json).unwrap();
        RoutableEvent::try_from(log).unwrap()
    }

    #[test]
    fn round_trip_amazon_time_sync() {
        let event = Ntp::builder()
            .counter_pre(TscCount::new(100))
            .counter_post(TscCount::new(200))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1000),
                server_send_time: Instant::new(2000),
                root_delay: Duration::new(50),
                root_dispersion: Duration::new(25),
                stratum: Stratum::ONE,
            })
            .build()
            .unwrap();

        let original = RoutableEvent::AmazonTimeSync(event);
        assert_eq!(original, round_trip(&original));
    }

    #[test]
    fn round_trip_ntp_source() {
        let event = Ntp::builder()
            .counter_pre(TscCount::new(100))
            .counter_post(TscCount::new(200))
            .ntp_data(NtpData {
                server_recv_time: Instant::new(1000),
                server_send_time: Instant::new(2000),
                root_delay: Duration::new(50),
                root_dispersion: Duration::new(25),
                stratum: Stratum::TWO,
            })
            .build()
            .unwrap();

        let addr: SocketAddr = "10.0.0.1:123".parse().unwrap();
        let original = RoutableEvent::NtpSource(addr, event);
        assert_eq!(original, round_trip(&original));
    }

    #[test]
    fn round_trip_phc() {
        let event = Phc::builder()
            .counter_pre(TscCount::new(300))
            .counter_post(TscCount::new(400))
            .data(PhcData {
                time: Instant::new(5000),
                clock_error_bound: Duration::new(100),
            })
            .build()
            .unwrap();

        let original = RoutableEvent::Phc(event);
        assert_eq!(original, round_trip(&original));
    }

    #[test]
    fn ntp_counters_out_of_order_errors() {
        // counter_post <= counter_pre must be rejected by the builder via TryFrom.
        let log = RoutableEventLog::Ntp {
            identifier: AMAZON_TIME_SYNC_ADDRESS_STR.to_string(),
            counter_pre: TscCount::new(200),
            counter_post: TscCount::new(100),
            data: NtpDataLog {
                server_recv_time_ns: Instant::new(1000),
                server_send_time_ns: Instant::new(2000),
                root_delay_ns: Duration::new(50),
                root_dispersion_ns: Duration::new(25),
                stratum: Stratum::ONE,
            },
        };
        let err = RoutableEvent::try_from(log).unwrap_err();
        assert_eq!(
            err.to_string(),
            "invalid NTP event: counter_post must be greater than counter_pre"
        );
    }

    #[test]
    fn invalid_socket_address_errors() {
        let log = RoutableEventLog::Ntp {
            identifier: "not-an-address".to_string(),
            counter_pre: TscCount::new(100),
            counter_post: TscCount::new(200),
            data: NtpDataLog {
                server_recv_time_ns: Instant::new(1000),
                server_send_time_ns: Instant::new(2000),
                root_delay_ns: Duration::new(50),
                root_dispersion_ns: Duration::new(25),
                stratum: Stratum::ONE,
            },
        };
        let err = RoutableEvent::try_from(log).unwrap_err();
        assert!(err.to_string().starts_with("invalid socket address:"));
    }

    // --- ClockParameters log conversion ---

    #[test]
    fn clock_parameters_round_trip_defaults_as_of_monotonic() {
        let log = ClockParametersLog {
            counter_sync: TscCount::new(150),
            time_sync_ns: Instant::new(1500),
            clock_error_bound_ns: Duration::new(75),
            period_s: Period::from_seconds(4.0e-10),
            period_max_error_s: Period::from_seconds(1.0e-16),
        };

        let params = ClockParameters::from(log.clone());
        assert_eq!(params.as_of_monotonic, Instant::default());

        // Re-projecting to the log drops `as_of_monotonic` and is lossless.
        assert_eq!(ClockParametersLog::from(&params), log);
    }
}
