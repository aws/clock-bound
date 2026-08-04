//! Test results aggregation and reporting.
//!
//! Determines the test result, serializes to JSON, and prints a summary.

use std::fs;

use serde::Serialize;

use crate::types::BoundCheck;

/// Overall test result.
///
/// Priority (first matching wins):
/// 1. No samples collected -> `NoSamplesCollected`
/// 2. >10% invalid clock state -> `ClockStateInvalid`
/// 3. Any combined bound failures -> `CombinedBoundAssertionFailure`
/// 4. Otherwise -> `Passed`
#[derive(Debug, Clone, Serialize)]
pub enum TestResult {
    Passed,
    NoSamplesCollected,
    CombinedBoundAssertionFailure(Vec<BoundCheck>),
    ClockStateInvalid(Vec<BoundCheck>),
}

/// Aggregated test results, serialized as the JSON artifact.
#[derive(Debug, Serialize)]
pub struct ValidationReport {
    pub total_samples: u64,
    pub shm_errors: u64,
    pub successful_combined_assertions: u64,
    pub failed_before_bound_assertions: u64,
    pub failed_after_bound_assertions: u64,
    pub failed_combined_bound_assertions: u64,
    pub failed_clockstate_assertions: u64,
    pub total_failed_assertions: u64,
    pub result: TestResult,
    pub checks: Vec<BoundCheck>,
}

impl ValidationReport {
    pub fn from_checks(checks: Vec<BoundCheck>, shm_errors: u64) -> Self {
        let total_samples = checks.len() as u64;

        let failed_clockstate_assertions =
            checks.iter().filter(|c| c.invalid_clockstate).count() as u64;
        let failed_before_bound_assertions = checks
            .iter()
            .filter(|c| !c.before_contains_realtime() && !c.invalid_clockstate)
            .count() as u64;
        let failed_after_bound_assertions = checks
            .iter()
            .filter(|c| !c.after_contains_realtime() && !c.invalid_clockstate)
            .count() as u64;
        let failed_combined_bound_assertions = checks
            .iter()
            .filter(|c| !c.combined_contains_realtime() && !c.invalid_clockstate)
            .count() as u64;

        let total_failed_assertions =
            failed_combined_bound_assertions + failed_clockstate_assertions;
        let successful_combined_assertions = total_samples - total_failed_assertions;

        // Precision loss is acceptable here — we're comparing a ratio against 0.1,
        // and sample counts won't exceed f64 mantissa precision in practice.
        #[allow(clippy::cast_precision_loss)]
        let result = if total_samples == 0 {
            TestResult::NoSamplesCollected
        } else if (failed_clockstate_assertions as f64 / total_samples as f64) > 0.1 {
            let failed = checks
                .iter()
                .filter(|c| c.invalid_clockstate)
                .cloned()
                .collect();
            TestResult::ClockStateInvalid(failed)
        } else if failed_combined_bound_assertions > 0 {
            let failed = checks
                .iter()
                .filter(|c| !c.combined_contains_realtime() && !c.invalid_clockstate)
                .cloned()
                .collect();
            TestResult::CombinedBoundAssertionFailure(failed)
        } else {
            TestResult::Passed
        };

        Self {
            total_samples,
            shm_errors,
            successful_combined_assertions,
            failed_before_bound_assertions,
            failed_after_bound_assertions,
            failed_combined_bound_assertions,
            failed_clockstate_assertions,
            total_failed_assertions,
            result,
            checks,
        }
    }

    pub fn write_json(&self, path: &str) {
        match serde_json::to_string_pretty(self) {
            Ok(json) => {
                if let Err(e) = fs::write(path, &json) {
                    eprintln!("WARNING: Failed to write results to '{path}': {e}");
                } else {
                    println!("Results written to: {path}");
                }
            }
            Err(e) => eprintln!("WARNING: Failed to serialize results: {e}"),
        }
    }

    pub fn print_summary(&self) {
        println!();
        println!("=== Result Summary ===");
        println!("{:<35} {}", "Total samples:", self.total_samples);
        println!("{:<35} {}", "SHM read errors:", self.shm_errors);
        println!(
            "{:<35} {}",
            "Successful combined assertions:", self.successful_combined_assertions
        );
        println!(
            "{:<35} {}",
            "Total failed assertions:", self.total_failed_assertions
        );
        println!(
            "  {:<33} {}",
            "Failed clock state:", self.failed_clockstate_assertions
        );
        println!(
            "  {:<33} {}",
            "Failed combined bound:", self.failed_combined_bound_assertions
        );
        println!(
            "  {:<33} {}",
            "Failed before bound:", self.failed_before_bound_assertions
        );
        println!(
            "  {:<33} {}",
            "Failed after bound:", self.failed_after_bound_assertions
        );
        println!();
    }
}
