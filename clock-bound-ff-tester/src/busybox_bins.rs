//! binary logic
#![expect(
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::missing_panics_doc,
    reason = "Treating these functions as main()"
)]

pub mod oscillator_generator;
pub mod primer_log_to_parquet;
pub mod repro_replay;
pub mod scenario_generator;
pub mod sim_amazon_time_sync;
