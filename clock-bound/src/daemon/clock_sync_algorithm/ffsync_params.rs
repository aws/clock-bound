//! Tunable parameters of the feed-forward synchronization algorithm.
//!
//! This module gathers every parameter that can be used by the synchronization algorithm.
//! Every parameter the algorithm has access to is listed here, with its production value as the
//! `Default`.
//!
//! The design principle of keeping the synchronization expertise to within the clock-bound daemon
//! does remain. The daemon MUST only construct [`FfSyncParams::default()`] and nothing else. This
//! is a key invariant to ensure that a version or commit id provides exact traceability to an
//! algorithm / model version, and the parameters it uses.
//!
//! On the other end, replaying existing ffevents log should allow for instrumentation and the
//! construction of a set of parameters with alternative values to explore the parameter space.
//!
//! Units are carried by the types (`Duration`, `Skew`).

use std::num::NonZeroUsize;

use crate::daemon::time::{Duration, tsc::Skew};

/// The maximum dispersion growth every second, in parts per billion, of the underlying oscillator.
///
/// In between updates, the clock error bound continuously grows to take into account the worse
/// case drift of the underlying oscillator. The maximum dispersion is the rate of growth applied
/// to the last clock error bound update.
///
/// If the value is 15,000 parts per billion, for example, then every second we go without an
/// updated measurement the clock error bound will increase by 15 microseconds.
///
/// This number is based on CPU spec sheet error tolerances
pub const DEFAULT_MAX_DISPERSION_PPB: u32 = 15_000;

/// Length of the simple-skew-model window: the largest period over which the local oscillator
/// is modelled as linear.
pub const DEFAULT_SKM_WINDOW: Duration = Duration::from_secs(1024);

/// Number of SKM windows of history kept for the long-term period estimate.
pub const DEFAULT_ESTIMATE_CAPACITY: NonZeroUsize = NonZeroUsize::new(600).unwrap();

/// Parameters shared by the whole algorithm plus one set per source type.
#[derive(Debug, Clone, PartialEq)]
pub struct FfSyncParams {
    /// Upper bound on local oscillator drift; grows clock error bounds with age.
    pub max_dispersion: Skew,
    /// Largest window over which the oscillator is modelled as linear. Sizes the SKM-window
    /// buffers and paces the estimate history (one best point per window).
    pub skm_window: Duration,
    /// SKM windows of history kept for the long-term period estimate.
    pub estimate_capacity: NonZeroUsize,
    // Sources are listed in the order of their expected accuracy, here and throughout the code:
    // PHC, then EC2 link-local NTP, then other NTP servers.
    /// PTP hardware clock.
    pub phc: SourceParams,
    /// The EC2 link-local NTP source (Amazon Time Sync): same estimator as `ntp`, own tuning.
    pub amazon_time_sync: SourceParams,
    /// Configured / public NTP servers.
    pub ntp: SourceParams,
}

/// Source specific parameters.
///
/// The two windows are durations measured back from the newest sample and must not exceed
/// [`FfSyncParams::skm_window`].
#[derive(Debug, Clone, PartialEq)]
pub struct SourceParams {
    /// `p_local` is computed from the samples within the last `period_window` (min-RTT sample of
    /// the oldest quarter against the newest quarter).
    pub period_window: Duration,
    /// `theta_hat` is computed from the samples within the last `theta_window`.
    pub theta_window: Duration,
    /// A sample whose RTT exceeds `multiplier x min RTT` (over the local buffer) is rejected.
    pub rtt_threshold_multiplier: usize,
    /// Weight of a sample in `theta_hat`: `exp(-sqrt(sample_error / factor))`. Penalises slow
    /// reads. `f64` because the arithmetic requires it.
    pub error_normalization_factor: f64,
}

impl SourceParams {
    /// Production values for the PHC estimator.
    /// The period and theta windows currently scoped at 100s.
    /// TODO: revisit window size after calibration.
    pub const PHC_DEFAULTS: Self = Self {
        period_window: Duration::from_secs(100),
        theta_window: Duration::from_secs(100),
        rtt_threshold_multiplier: 5,
        error_normalization_factor: 1e5,
    };

    /// Production values for the NTP estimator.
    /// The period and theta windows cover the whole `SKM_WINDOW`.
    /// TODO: this is currently used by both the EC2 Link local, and any NTP source. To revisit.
    pub const NTP_DEFAULTS: Self = Self {
        period_window: DEFAULT_SKM_WINDOW,
        theta_window: DEFAULT_SKM_WINDOW,
        rtt_threshold_multiplier: 5,
        error_normalization_factor: 1e5,
    };

    /// The window the local buffer must hold: the larger of the two.
    pub fn local_window(&self) -> Duration {
        std::cmp::max(self.period_window, self.theta_window)
    }
}

impl Default for FfSyncParams {
    fn default() -> Self {
        Self {
            max_dispersion: Skew::from_ppb(f64::from(DEFAULT_MAX_DISPERSION_PPB)),
            skm_window: DEFAULT_SKM_WINDOW,
            estimate_capacity: DEFAULT_ESTIMATE_CAPACITY,
            phc: SourceParams::PHC_DEFAULTS,
            amazon_time_sync: SourceParams::NTP_DEFAULTS,
            ntp: SourceParams::NTP_DEFAULTS,
        }
    }
}

/// Why a set of parameters is unusable.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FfSyncParamsError {
    #[error("skm_window must be positive, got {0:?}")]
    SkmWindowNotPositive(Duration),
    #[error("max_dispersion must be positive")]
    MaxDispersionNotPositive,
    #[error("{source_type}: {window} ({value:?}) must be positive")]
    WindowNotPositive {
        source_type: &'static str,
        window: &'static str,
        value: Duration,
    },
    #[error("{source_type}: {window} ({value:?}) exceeds skm_window ({skm_window:?})")]
    WindowExceedsSkm {
        source_type: &'static str,
        window: &'static str,
        value: Duration,
        skm_window: Duration,
    },
    #[error(
        "{source_type}: local window ({value:?}) must hold at least {min_samples} samples at a {poll:?} poll interval, i.e. be at least {min:?}"
    )]
    WindowTooShort {
        source_type: &'static str,
        value: Duration,
        poll: Duration,
        min_samples: u32,
        min: Duration,
    },
    #[error("{source_type}: rtt_threshold_multiplier must be at least 1")]
    RttMultiplierZero { source_type: &'static str },
    #[error("{source_type}: error_normalization_factor must be positive and finite, got {value}")]
    NormalizationFactorInvalid {
        source_type: &'static str,
        value: f64,
    },
}

/// Fewest samples the quarter logic of `p_local` needs in a window.
const MIN_SAMPLES_PER_WINDOW: u32 = 4;

impl FfSyncParams {
    /// Check the parameters against the constraints the estimators rely on.
    ///
    /// The defaults, used by the daemon or other satisfy the constraints by construction
    /// (see the test below).
    ///
    /// Tooling that passes specific parameter values (e.g. replaying existing traces) should
    /// first validate the parameters they are supplied with.
    ///
    /// The sources' poll intervals (`source::POLL_INTERVALS`, in the same order as the fields)
    /// size the buffers and bound the windows from below.
    ///
    /// # Errors
    /// The first constraint violated, as a [`FfSyncParamsError`] naming the source type, the field
    /// and the offending value.
    pub fn validate(&self) -> Result<(), FfSyncParamsError> {
        let polls = super::source::POLL_INTERVALS;
        if self.skm_window <= Duration::ZERO {
            return Err(FfSyncParamsError::SkmWindowNotPositive(self.skm_window));
        }
        if self.max_dispersion.get() <= 0.0 {
            return Err(FfSyncParamsError::MaxDispersionNotPositive);
        }
        let sources = [
            ("phc", &self.phc, polls[0]),
            ("amazon_time_sync", &self.amazon_time_sync, polls[1]),
            ("ntp", &self.ntp, polls[2]),
        ];
        for (name, source, poll) in sources {
            for (window_name, value) in [
                ("period_window", source.period_window),
                ("theta_window", source.theta_window),
            ] {
                if value <= Duration::ZERO {
                    return Err(FfSyncParamsError::WindowNotPositive {
                        source_type: name,
                        window: window_name,
                        value,
                    });
                }
                if value > self.skm_window {
                    return Err(FfSyncParamsError::WindowExceedsSkm {
                        source_type: name,
                        window: window_name,
                        value,
                        skm_window: self.skm_window,
                    });
                }
            }
            let min = Duration::from_nanos(poll.as_nanos() * i64::from(MIN_SAMPLES_PER_WINDOW));
            if source.local_window() < min {
                return Err(FfSyncParamsError::WindowTooShort {
                    source_type: name,
                    value: source.local_window(),
                    poll,
                    min_samples: MIN_SAMPLES_PER_WINDOW,
                    min,
                });
            }
            if source.rtt_threshold_multiplier == 0 {
                return Err(FfSyncParamsError::RttMultiplierZero { source_type: name });
            }
            let f = source.error_normalization_factor;
            if !(f.is_finite() && f > 0.0) {
                return Err(FfSyncParamsError::NormalizationFactorInvalid {
                    source_type: name,
                    value: f,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_match_production_literals() {
        let p = FfSyncParams::default();
        p.validate().unwrap();
        // The daemon's value. Note that `Skew::from_ppm(15.0)` is one ulp away from it; the
        // tester used to construct that one, so its replays were one ulp off the daemon.
        assert_eq!(p.max_dispersion, Skew::from_ppb(15_000.0));
        assert_eq!(p.skm_window, Duration::from_secs(1024));
        assert_eq!(p.estimate_capacity.get(), 600);
        assert_eq!(p.ntp.period_window, Duration::from_secs(1024));
        assert_eq!(p.amazon_time_sync.theta_window, Duration::from_secs(1024));
        assert_eq!(p.phc.period_window, Duration::from_secs(100));
        assert_eq!(p.phc.theta_window, Duration::from_secs(100));
        assert_eq!(p.phc.local_window(), Duration::from_secs(100));
        assert_eq!(p.ntp.rtt_threshold_multiplier, 5);
        assert_eq!(p.phc.error_normalization_factor, 1e5);
    }

    #[test]
    fn window_exceeding_skm_is_rejected() {
        let mut p = FfSyncParams::default();
        p.phc.theta_window = Duration::from_secs(2048);
        assert!(matches!(
            p.validate(),
            Err(FfSyncParamsError::WindowExceedsSkm {
                source_type: "phc",
                window: "theta_window",
                ..
            })
        ));
    }

    #[test]
    fn window_too_short_for_poll_is_rejected() {
        let mut p = FfSyncParams::default();
        p.ntp.period_window = Duration::from_secs(30);
        p.ntp.theta_window = Duration::from_secs(30);
        // 4 samples at the NTP poll interval of 16 s need 64 s.
        assert!(matches!(
            p.validate(),
            Err(FfSyncParamsError::WindowTooShort {
                source_type: "ntp",
                ..
            })
        ));
        p.ntp.theta_window = Duration::from_secs(64);
        p.validate().unwrap();
    }

    #[test]
    fn zero_and_invalid_values_are_rejected() {
        let mut p = FfSyncParams::default();
        p.amazon_time_sync.rtt_threshold_multiplier = 0;
        assert!(matches!(
            p.validate(),
            Err(FfSyncParamsError::RttMultiplierZero {
                source_type: "amazon_time_sync"
            })
        ));
        let mut p = FfSyncParams::default();
        p.phc.error_normalization_factor = f64::NAN;
        assert!(matches!(
            p.validate(),
            Err(FfSyncParamsError::NormalizationFactorInvalid {
                source_type: "phc",
                ..
            })
        ));
        let mut p = FfSyncParams::default();
        p.skm_window = Duration::ZERO;
        assert!(matches!(
            p.validate(),
            Err(FfSyncParamsError::SkmWindowNotPositive(_))
        ));
    }
}
