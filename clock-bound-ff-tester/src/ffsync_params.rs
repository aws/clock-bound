//! Parameter files for replay: the file schema, its conversion into [`FfSyncParams`], and the
//! hash that identifies a non-default parameter set.
//!
//! The algorithm crate holds the *typed* parameters (`Duration`, `Skew`) and no parser: the
//! daemon can only ever construct [`FfSyncParams::default()`]. This module holds the *file
//! schema* — plain integers with unit suffixes in their names, so a value can never be misread —
//! and converts it into the typed struct after validation.
//!
//! The schema is strict: every key is required and unknown keys are an error, so a file says
//! exactly what it ran with and its meaning does not depend on the defaults of the binary it is
//! fed to. `clock-bound-ff-tester default-ffsync-params` prints the complete production file to
//! start from.
//!
//! ```toml
//! max_dispersion_ppb = 15000
//! skm_window_s = 1024
//! estimate_capacity = 600
//!
//! [phc]
//! period_window_s = 100
//! theta_window_s = 100
//! rtt_threshold_multiplier = 5
//! error_normalization_factor = 100000
//!
//! [amazon_time_sync]
//! period_window_s = 1024
//! theta_window_s = 1024
//! rtt_threshold_multiplier = 5
//! error_normalization_factor = 100000
//!
//! [ntp]
//! period_window_s = 1024
//! theta_window_s = 1024
//! rtt_threshold_multiplier = 5
//! error_normalization_factor = 100000
//! ```
//!
//! # Identity
//!
//! A replay made with parameters that differ from the default carries `+params-<hash>` in its
//! version string, where the hash is the first 8 hex digits of the SHA-256 of a canonical
//! rendering of the *effective* typed parameters (fixed key order, the integers above, no
//! floats). Two files spelling the same values are the same variant; a file spelling out the
//! defaults is the default variant, with no suffix. The effective parameters are also written as
//! the first record of both structured streams (`event = "params"`), so every log is
//! self-describing without access to the file or to git.

use std::{fmt::Write as _, path::Path};

use anyhow::Context;
use clock_bound::daemon::{
    clock_sync_algorithm::{FfSyncParams, SourceParams},
    logging::StructuredLog,
    time::{Duration, tsc::Skew},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Number of hex digits of the SHA-256 kept in the version suffix.
const HASH_HEX_LEN: usize = 8;

// `tracing`'s `target:` must be a constant expression, hence one constant per stream.
const FFEVENTS_TARGET: &str = StructuredLog::FFEvents.target();
const ALGO_ANALYSIS_TARGET: &str = StructuredLog::AlgoAnalysis.target();

/// The parameter file, and the effective parameters in the file's vocabulary.
///
/// One type for both directions: a file parses into it, and a typed [`FfSyncParams`] renders
/// back into it (see [`FfSyncParamsFile::from`]). Every key is required. Field order is the
/// canonical order and is preserved on serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FfSyncParamsFile {
    pub max_dispersion_ppb: u32,
    pub skm_window_s: u64,
    pub estimate_capacity: usize,
    // Sources in the order of their expected accuracy, as everywhere in the code.
    pub phc: SourceParamsFile,
    pub amazon_time_sync: SourceParamsFile,
    pub ntp: SourceParamsFile,
}

/// One source type's table in the parameter file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceParamsFile {
    pub period_window_s: u64,
    pub theta_window_s: u64,
    pub rtt_threshold_multiplier: usize,
    pub error_normalization_factor: u64,
}

/// Read a parameter file, validate it, and return the typed parameters.
///
/// # Errors
/// Unreadable or malformed file, a missing or unknown key, a value that does not fit its type,
/// or a combination the estimators cannot run with (see `FfSyncParams::validate`).
pub fn load(path: &Path) -> anyhow::Result<FfSyncParams> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading parameter file {}", path.display()))?;
    let file: FfSyncParamsFile = toml::from_str(&text)
        .with_context(|| format!("parsing parameter file {}", path.display()))?;
    let params = FfSyncParams::try_from(&file)?;
    params
        .validate()
        .with_context(|| format!("invalid parameters in {}", path.display()))?;
    Ok(params)
}

impl TryFrom<&FfSyncParamsFile> for FfSyncParams {
    type Error = anyhow::Error;

    /// Convert the file's integers into the typed parameters.
    ///
    /// Fails on a value that does not fit its typed counterpart (a window too long for
    /// `Duration`, a zero estimate capacity); the estimators' constraints are `validate`'s job.
    fn try_from(file: &FfSyncParamsFile) -> anyhow::Result<Self> {
        let source = |f: &SourceParamsFile| -> anyhow::Result<SourceParams> {
            Ok(SourceParams {
                period_window: seconds(f.period_window_s, "period_window_s")?,
                theta_window: seconds(f.theta_window_s, "theta_window_s")?,
                rtt_threshold_multiplier: f.rtt_threshold_multiplier,
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "factors are small integers (1e5 in production); f64 is exact far beyond them"
                )]
                error_normalization_factor: f.error_normalization_factor as f64,
            })
        };
        Ok(Self {
            max_dispersion: Skew::from_ppb(f64::from(file.max_dispersion_ppb)),
            skm_window: seconds(file.skm_window_s, "skm_window_s")?,
            estimate_capacity: file
                .estimate_capacity
                .try_into()
                .context("estimate_capacity must be at least 1")?,
            phc: source(&file.phc)?,
            amazon_time_sync: source(&file.amazon_time_sync)?,
            ntp: source(&file.ntp)?,
        })
    }
}

fn seconds(value: u64, key: &str) -> anyhow::Result<Duration> {
    let s = i64::try_from(value).with_context(|| format!("{key} is too large"))?;
    Ok(Duration::from_secs(s))
}

impl From<&FfSyncParams> for FfSyncParamsFile {
    /// Render the effective typed parameters in the file's integer vocabulary.
    ///
    /// # Panics
    /// If `max_dispersion` is not representable in ppb (`validate` rejects such values) or a
    /// window is not a whole number of seconds (the file schema only produces whole seconds).
    fn from(params: &FfSyncParams) -> Self {
        let source = |s: &SourceParams| SourceParamsFile {
            period_window_s: whole_seconds(s.period_window, "period_window"),
            theta_window_s: whole_seconds(s.theta_window, "theta_window"),
            rtt_threshold_multiplier: s.rtt_threshold_multiplier,
            error_normalization_factor: whole_factor(s.error_normalization_factor),
        };
        Self {
            max_dispersion_ppb: params
                .max_dispersion
                .to_ppb()
                .expect("max_dispersion is representable in ppb"),
            skm_window_s: whole_seconds(params.skm_window, "skm_window"),
            estimate_capacity: params.estimate_capacity.get(),
            phc: source(&params.phc),
            amazon_time_sync: source(&params.amazon_time_sync),
            ntp: source(&params.ntp),
        }
    }
}

#[expect(
    clippy::cast_sign_loss,
    reason = "durations here are positive (validated)"
)]
fn whole_seconds(d: Duration, what: &str) -> u64 {
    assert!(
        d > Duration::ZERO && d.as_nanos() % 1_000_000_000 == 0,
        "{what} must be a positive whole number of seconds, got {d:?}"
    );
    d.as_seconds() as u64
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked integral and non-negative first"
)]
fn whole_factor(f: f64) -> u64 {
    assert!(
        f.is_finite() && f >= 0.0 && f.fract() == 0.0,
        "error_normalization_factor must be a non-negative integer, got {f}"
    );
    f as u64
}

/// The complete production parameter file, as TOML: what `default-ffsync-params` prints.
///
/// # Panics
/// If the schema cannot be serialized to TOML, which would be a bug in this module.
pub fn default_file() -> String {
    let file = FfSyncParamsFile::from(&FfSyncParams::default());
    let body = toml::to_string(&file).expect("the schema serializes to TOML");
    format!(
        "# Feed-forward synchronization algorithm parameters, production values.\n\
         # Every key is required. Durations are whole seconds; dispersion is in parts per billion.\n\
         # Sources appear in the order of their expected accuracy: PHC, EC2 link-local NTP, other NTP.\n\
         \n{body}"
    )
}

/// Canonical rendering of the effective parameters: one line per key, fixed order, integers only.
/// This is the hashed text.
pub fn canonical(params: &FfSyncParams) -> String {
    let e = FfSyncParamsFile::from(params);
    let mut out = String::new();
    let _ = writeln!(out, "max_dispersion_ppb={}", e.max_dispersion_ppb);
    let _ = writeln!(out, "skm_window_s={}", e.skm_window_s);
    let _ = writeln!(out, "estimate_capacity={}", e.estimate_capacity);
    for (name, s) in [
        ("phc", &e.phc),
        ("amazon_time_sync", &e.amazon_time_sync),
        ("ntp", &e.ntp),
    ] {
        let _ = writeln!(out, "{name}.period_window_s={}", s.period_window_s);
        let _ = writeln!(out, "{name}.theta_window_s={}", s.theta_window_s);
        let _ = writeln!(
            out,
            "{name}.rtt_threshold_multiplier={}",
            s.rtt_threshold_multiplier
        );
        let _ = writeln!(
            out,
            "{name}.error_normalization_factor={}",
            s.error_normalization_factor
        );
    }
    out
}

/// First [`HASH_HEX_LEN`] hex digits of the SHA-256 of [`canonical`].
pub fn hash(params: &FfSyncParams) -> String {
    let digest = Sha256::digest(canonical(params).as_bytes());
    let mut hex = String::with_capacity(HASH_HEX_LEN);
    for byte in digest.iter().take(HASH_HEX_LEN / 2) {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// `+params-<hash>` when the parameters differ from the production defaults, `None` otherwise —
/// so a default replay carries exactly the version string the daemon carries.
pub fn version_suffix(params: &FfSyncParams) -> Option<String> {
    (*params != FfSyncParams::default()).then(|| format!("+params-{}", hash(params)))
}

/// Write the effective parameters as the first record of both structured streams, so every
/// replay log states what it was made with.
///
/// Must be called after the tracing subscriber is initialized and before any other event.
pub fn log_effective(params: &FfSyncParams) {
    #[derive(Serialize)]
    struct Body {
        #[serde(flatten)]
        params: FfSyncParamsFile,
        hash: String,
        default: bool,
    }
    let body = Body {
        params: FfSyncParamsFile::from(params),
        hash: hash(params),
        default: *params == FfSyncParams::default(),
    };
    let record = serde_json::json!({ "event": "params", "params": body });
    let Ok(json) = serde_json::to_string(&record) else {
        return;
    };
    // The layer reads the entry through `record_str`: pass a `&str`, not a `Display` value.
    let entry: &str = &json;
    tracing::info!(target: FFEVENTS_TARGET, entry = entry);
    tracing::info!(target: ALGO_ANALYSIS_TARGET, entry = entry);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_toml() -> String {
        toml::to_string(&FfSyncParamsFile::from(&FfSyncParams::default())).unwrap()
    }

    #[test]
    fn the_default_file_round_trips_to_the_defaults_with_no_suffix() {
        let file: FfSyncParamsFile = toml::from_str(&default_file()).unwrap();
        let params = FfSyncParams::try_from(&file).unwrap();
        assert_eq!(params, FfSyncParams::default());
        assert!(version_suffix(&params).is_none());
        assert_eq!(FfSyncParamsFile::from(&params), file);
    }

    #[test]
    fn the_default_file_lists_the_production_values_in_order() {
        let text = default_file();
        assert!(text.contains("max_dispersion_ppb = 15000\n"));
        assert!(text.contains("skm_window_s = 1024\n"));
        assert!(text.contains("estimate_capacity = 600\n"));
        let phc = text.find("[phc]").unwrap();
        let ats = text.find("[amazon_time_sync]").unwrap();
        let ntp = text.find("[ntp]").unwrap();
        assert!(phc < ats && ats < ntp, "sources in accuracy order");
        assert!(text[phc..ats].contains("period_window_s = 100\n"));
        assert!(text[ntp..].contains("theta_window_s = 1024\n"));
    }

    #[test]
    fn canonical_is_stable_and_hash_is_8_hex() {
        let c = canonical(&FfSyncParams::default());
        assert!(c.starts_with(
            "max_dispersion_ppb=15000\nskm_window_s=1024\nestimate_capacity=600\nphc."
        ));
        assert_eq!(c.lines().count(), 3 + 3 * 4);
        let h = hash(&FfSyncParams::default());
        assert_eq!(h.len(), 8);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn a_changed_value_changes_the_hash_and_adds_a_suffix() {
        let text = default_toml().replace("theta_window_s = 100", "theta_window_s = 50");
        let file: FfSyncParamsFile = toml::from_str(&text).unwrap();
        let params = FfSyncParams::try_from(&file).unwrap();
        assert_eq!(params.phc.theta_window, Duration::from_secs(50));
        assert_ne!(params, FfSyncParams::default());
        assert_ne!(hash(&params), hash(&FfSyncParams::default()));
        let suffix = version_suffix(&params).unwrap();
        assert!(suffix.starts_with("+params-") && suffix.len() == "+params-".len() + 8);
        // Same values, top-level keys in another order and with a comment: same hash.
        let reordered = format!(
            "# a comment\nskm_window_s = 1024\n{}",
            text.replace("skm_window_s = 1024\n", "")
        );
        let file2: FfSyncParamsFile = toml::from_str(&reordered).unwrap();
        assert_eq!(
            hash(&FfSyncParams::try_from(&file2).unwrap()),
            hash(&params)
        );
    }

    #[test]
    fn missing_and_unknown_keys_are_rejected() {
        let missing = default_toml().replace("estimate_capacity = 600\n", "");
        let err = toml::from_str::<FfSyncParamsFile>(&missing).unwrap_err();
        assert!(err.to_string().contains("estimate_capacity"), "{err}");
        let missing_source_key = default_toml().replacen("rtt_threshold_multiplier = 5\n", "", 1);
        assert!(toml::from_str::<FfSyncParamsFile>(&missing_source_key).is_err());
        let unknown = default_toml() + "skm_window = 5\n";
        let err = toml::from_str::<FfSyncParamsFile>(&unknown).unwrap_err();
        assert!(err.to_string().contains("skm_window"), "{err}");
    }

    #[test]
    fn floats_and_negatives_are_rejected_by_the_schema() {
        let base = default_toml();
        assert!(
            toml::from_str::<FfSyncParamsFile>(
                &base.replace("skm_window_s = 1024", "skm_window_s = 1024.5")
            )
            .is_err()
        );
        assert!(
            toml::from_str::<FfSyncParamsFile>(&base.replacen(
                "error_normalization_factor = 100000",
                "error_normalization_factor = 1e5",
                1
            ))
            .is_err()
        );
        assert!(
            toml::from_str::<FfSyncParamsFile>(
                &base.replace("max_dispersion_ppb = 15000", "max_dispersion_ppb = -1")
            )
            .is_err()
        );
    }

    #[test]
    fn invalid_combinations_are_refused_by_validate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.toml");
        std::fs::write(
            &path,
            default_toml().replacen("theta_window_s = 100", "theta_window_s = 2048", 1),
        )
        .unwrap();
        let err = load(&path).unwrap_err();
        assert!(format!("{err:#}").contains("exceeds skm_window"), "{err:#}");
        std::fs::write(
            &path,
            default_toml().replace("estimate_capacity = 600", "estimate_capacity = 0"),
        )
        .unwrap();
        assert!(load(&path).is_err());
    }

    #[test]
    fn load_reads_a_real_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.toml");
        let text = default_toml()
            .replace("max_dispersion_ppb = 15000", "max_dispersion_ppb = 20000")
            .replace(
                "rtt_threshold_multiplier = 5\n",
                "rtt_threshold_multiplier = 3\n",
            );
        std::fs::write(&path, text).unwrap();
        let params = load(&path).unwrap();
        assert_eq!(params.max_dispersion, Skew::from_ppb(20_000.0));
        assert_eq!(params.ntp.rtt_threshold_multiplier, 3);
        assert_eq!(params.phc.rtt_threshold_multiplier, 3);
        assert_eq!(
            params.phc.period_window,
            FfSyncParams::default().phc.period_window
        );
    }
}
