//! Oscillator Generator CLI Tool
//!
//! This tool generates simple oscillator models for simulation. It supports:
//! - Simple oscillator: Creates oscillators with constant skew and offset
//! - Sine wave oscillator: Creates oscillators with sinusoidal offset patterns
//!
//! The generated oscillator models can be saved to a file or output to stdout in JSON format.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use clock_bound_ff_tester::simulation::oscillator::{Noise, Oscillator};
use clock_bound_ff_tester::time::{Frequency, Skew, TrueDuration, TrueInstant, TscCount};
use rand::SeedableRng;

/// Main command-line interface for the oscillator generator
///
/// This struct defines the top-level CLI interface with subcommands for
/// different oscillator types and global options.
#[derive(Clone, Debug, Parser)]
pub struct Cli {
    #[command(subcommand)]
    /// Subcommand for this CLI
    pub command: Commands,
}

/// Available subcommands for oscillator generation
///
/// Each variant corresponds to a different type of oscillator model
/// that can be generated using this tool.
#[derive(Clone, Debug, Subcommand)]
pub enum Commands {
    /// Generate a simple oscillator model
    ///
    /// Creates an oscillator with constant skew and offset
    Simple(Simple),

    /// Generate a sinusoidal oscillator model
    ///
    /// Creates an oscillator with a sinusoidal offset pattern that oscillates
    /// with a specified period and amplitude.
    Sine(Sine),
}

/// Configuration for noise parameters
///
/// Defines the parameters for adding random noise to oscillator models
#[derive(Clone, Debug, Parser)]
pub struct NoiseArgs {
    #[arg(long)]
    /// Enable noise simulation in the oscillator model
    pub noise: bool,

    #[arg(long, default_value = "0s", requires = "noise")]
    /// Mean value of the noise distribution (e.g. 50microseconds)
    pub noise_mean: TrueDuration,

    #[arg(long, required_if_eq("noise", "true"), requires = "noise")]
    /// Standard deviation of the noise distribution (e.g. 30microseconds)
    /// Required when noise is enabled
    pub noise_std_dev: Option<TrueDuration>,

    #[arg(long, default_value = "1s", requires = "noise")]
    /// Time interval between noise samples
    pub noise_step: TrueDuration,

    #[arg(long, requires = "noise")]
    /// Seed for reproducible noise generation (omit to use default RNG behavior)
    pub noise_seed: Option<u64>,
}

/// Configuration for simple oscillator models
///
/// Defines parameters specific to simple oscillators
#[derive(Clone, Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct Simple {
    #[arg(long, default_value = "0ppm")]
    /// The skew of the local oscillator (parts per million)
    pub skew: Skew,

    #[arg(long, default_value = "0us")]
    /// The starting local oscillator offset from true time
    pub start_offset: TrueDuration,

    #[command(flatten)]
    /// Common oscillator parameters
    pub common: CommonArgs,

    #[command(flatten)]
    /// Noise parameters
    pub noise: NoiseArgs,
}

/// Configuration for sinusoidal oscillator models
///
/// Defines parameters specific to sine wave oscillators
#[derive(Clone, Debug, Parser)]
#[command(version, about, long_about = None)]
pub struct Sine {
    #[arg(long, default_value = "5minutes")]
    /// The period of the oscillator walk oscillation
    pub period: TrueDuration,

    #[arg(long, default_value = "40microseconds")]
    /// The offset at the peak of the sine wave oscillation
    pub amplitude: TrueDuration,

    #[arg(long, default_value = "1s")]
    /// Sample period of the oscillator model
    pub sample_period: TrueDuration,

    #[command(flatten)]
    /// Common oscillator parameters
    pub common: CommonArgs,

    #[command(flatten)]
    /// Noise parameters
    pub noise: NoiseArgs,
}

/// Parameters common to all oscillator types
///
/// These parameters define the basic properties shared by all oscillator models
#[derive(Clone, Debug, Parser)]
pub struct CommonArgs {
    #[arg(short, long, default_value = "1ghz")]
    /// The nominal (or starting) clock frequency of the local oscillator
    pub clock_frequency: Frequency,

    #[arg(long, default_value = "9000days")]
    /// The start time of the scenario
    pub start_time: TrueInstant,

    #[arg(long, default_value = "5minutes")]
    /// The duration of the scenario
    pub duration: TrueDuration,

    #[arg(long, default_value = "0")]
    /// The tsc timestamp at the start of the scenario
    pub tsc_timestamp_start: TscCount,

    #[arg(short, long)]
    /// Optional output file name, defaults to stdout
    pub output: Option<PathBuf>,
}

/// Main entry point for the oscillator generator tool
///
/// Parses command-line arguments, creates a simple oscillator model,
/// and outputs the model to the specified file or to stdout.
fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let (oscillator, output) = match &cli.command {
        Commands::Simple(args) => handle_simple_command(args),
        Commands::Sine(args) => handle_sine_command(args),
    };

    serialize_oscillator(&oscillator, output.as_ref())
}

/// Creates a noise model if noise is enabled
fn create_noise_model(args: &NoiseArgs) -> Option<Noise> {
    if !args.noise {
        return None;
    }

    // noise_std_dev will always be Some if --noise is true due to required_if_eq
    let std_dev = args
        .noise_std_dev
        .expect("noise_std_dev should be set when noise is enabled");

    let rng = match args.noise_seed {
        None => {
            // Use RNG seeded from os entropy
            Box::new(rand_chacha::ChaCha12Rng::from_rng(rand::rngs::OsRng).unwrap())
        }
        Some(seed) => {
            // Use the specified seed
            Box::new(rand_chacha::ChaCha12Rng::seed_from_u64(seed))
        }
    };

    // Create the noise model
    Some(
        Noise::builder()
            .rng(rng)
            .mean(args.noise_mean)
            .std_dev(std_dev)
            .step_size(args.noise_step)
            .build(),
    )
}

/// Handles the Simple oscillator command
fn handle_simple_command(args: &Simple) -> (Oscillator, Option<PathBuf>) {
    let noise_opt = create_noise_model(&args.noise);

    (
        Oscillator::create_simple()
            .clock_frequency(args.common.clock_frequency)
            .start_time(args.common.start_time)
            .duration(args.common.duration)
            .tsc_timestamp_start(args.common.tsc_timestamp_start)
            .skew(args.skew)
            .starting_oscillator_offset(args.start_offset)
            .maybe_noise(noise_opt)
            .call(),
        args.common.output.clone(),
    )
}

/// Handles the Sine oscillator command
fn handle_sine_command(args: &Sine) -> (Oscillator, Option<PathBuf>) {
    let noise_opt = create_noise_model(&args.noise);

    (
        Oscillator::create_sin()
            .clock_frequency(args.common.clock_frequency)
            .start_time(args.common.start_time)
            .duration(args.common.duration)
            .tsc_timestamp_start(args.common.tsc_timestamp_start)
            .period(args.period)
            .amplitude(args.amplitude)
            .sample_period(args.sample_period)
            .maybe_noise(noise_opt)
            .call(),
        args.common.output.clone(),
    )
}

/// Outputs the oscillator model to file or stdout
fn serialize_oscillator(oscillator: &Oscillator, output: Option<&PathBuf>) -> anyhow::Result<()> {
    if let Some(output_path) = output.as_ref() {
        let mut file = std::fs::File::create(output_path)?;
        serde_json::to_writer_pretty(&mut file, &oscillator.inner)?;
        println!("Wrote to {}", output_path.display());
    } else {
        let mut out = std::io::stdout().lock();
        serde_json::to_writer_pretty(&mut out, &oscillator.inner)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests CLI argument parsing for simple oscillator configuration
    #[test]
    fn oscillator_simple_cli_parsing() {
        let args = vec![
            "test",
            "simple",
            "--skew",
            "15ppm",
            "--start-offset",
            "25us",
            "--clock-frequency",
            "2ghz",
            "--duration",
            "15minutes",
        ];

        let cli = Cli::try_parse_from(args).unwrap();

        if let Commands::Simple(simple_args) = &cli.command {
            assert_eq!(simple_args.skew, Skew::from_ppm(15.0));
            assert_eq!(simple_args.start_offset, TrueDuration::from_micros(25));
            assert_eq!(simple_args.common.clock_frequency, Frequency::from_ghz(2.0));
            assert_eq!(simple_args.common.duration, TrueDuration::from_minutes(15));
        } else {
            panic!("Wrong command parsed")
        }
    }

    /// Tests default values for simple oscillator configuration
    #[test]
    fn oscillator_simple_default_values() {
        let args = vec!["test", "simple"];
        let cli = Cli::try_parse_from(args).unwrap();

        if let Commands::Simple(simple_args) = &cli.command {
            assert_eq!(simple_args.skew, Skew::from_ppm(0.0));
            assert_eq!(simple_args.start_offset, TrueDuration::from_micros(0));
            assert_eq!(simple_args.common.clock_frequency, Frequency::from_ghz(1.0));
            assert_eq!(simple_args.common.duration, TrueDuration::from_minutes(5));
            assert_eq!(simple_args.common.tsc_timestamp_start, TscCount::new(0));
            assert_eq!(simple_args.common.output, None);

            // Check default noise parameters
            assert!(!simple_args.noise.noise);
            assert_eq!(simple_args.noise.noise_mean, TrueDuration::from_secs(0));
            assert_eq!(simple_args.noise.noise_std_dev, None);
            assert_eq!(simple_args.noise.noise_step, TrueDuration::from_secs(1));
            assert_eq!(simple_args.noise.noise_seed, None);
        } else {
            panic!("Wrong command parsed")
        }
    }

    /// Tests CLI argument parsing for simple oscillator with noise
    #[test]
    fn oscillator_simple_with_noise_cli_parsing() {
        let args = vec![
            "test",
            "simple",
            "--noise",
            "--noise-mean",
            "5us",
            "--noise-std-dev",
            "10us",
            "--noise-step",
            "5ms",
            "--noise-seed",
            "123",
        ];

        let cli = Cli::try_parse_from(args).unwrap();

        if let Commands::Simple(simple_args) = &cli.command {
            assert!(simple_args.noise.noise);
            assert_eq!(simple_args.noise.noise_mean, TrueDuration::from_micros(5));
            assert_eq!(
                simple_args.noise.noise_std_dev,
                Some(TrueDuration::from_micros(10))
            );
            assert_eq!(simple_args.noise.noise_step, TrueDuration::from_millis(5));
            assert_eq!(simple_args.noise.noise_seed, Some(123));
        } else {
            panic!("Wrong command parsed")
        }
    }

    /// Tests that noise parameters require the noise flag
    #[test]
    fn oscillator_simple_noise_params_require_noise_flag() {
        // Test with --noise-mean but without --noise
        let args = vec!["test", "simple", "--noise-mean", "5us"];

        let result = Cli::try_parse_from(args);
        assert!(
            result.is_err(),
            "Should error when specifying noise params without --noise flag"
        );

        // Test with --noise-std-dev but without --noise
        let args = vec!["test", "simple", "--noise-std-dev", "10us"];

        let result = Cli::try_parse_from(args);
        assert!(
            result.is_err(),
            "Should error when specifying noise params without --noise flag"
        );

        // Test with --noise-step but without --noise
        let args = vec!["test", "simple", "--noise-step", "5ms"];

        let result = Cli::try_parse_from(args);
        assert!(
            result.is_err(),
            "Should error when specifying noise params without --noise flag"
        );

        // Test with --noise-seed but without --noise
        let args = vec!["test", "simple", "--noise-seed", "123"];

        let result = Cli::try_parse_from(args);
        assert!(
            result.is_err(),
            "Should error when specifying noise-seed without --noise flag"
        );
    }

    /// Tests CLI argument parsing for sine oscillator configuration
    #[test]
    fn oscillator_sine_cli_parsing() {
        let args = vec![
            "test",
            "sine",
            "--period",
            "10minutes",
            "--amplitude",
            "50microseconds",
            "--sample-period",
            "2s",
            "--clock-frequency",
            "2ghz",
            "--duration",
            "15minutes",
        ];

        let cli = Cli::try_parse_from(args).unwrap();

        if let Commands::Sine(sine_args) = &cli.command {
            assert_eq!(sine_args.period, TrueDuration::from_minutes(10));
            assert_eq!(sine_args.amplitude, TrueDuration::from_micros(50));
            assert_eq!(sine_args.sample_period, TrueDuration::from_secs(2));
            assert_eq!(sine_args.common.clock_frequency, Frequency::from_ghz(2.0));
            assert_eq!(sine_args.common.duration, TrueDuration::from_minutes(15));
        } else {
            panic!("Wrong command parsed")
        }
    }

    /// Tests CLI argument parsing for sine oscillator with noise
    #[test]
    fn oscillator_sine_with_noise_cli_parsing() {
        let args = vec![
            "test",
            "sine",
            "--noise",
            "--noise-mean",
            "2500ns",
            "--noise-std-dev",
            "7500ns",
            "--noise-step",
            "20ms",
            "--noise-seed",
            "456",
        ];

        let cli = Cli::try_parse_from(args).unwrap();

        if let Commands::Sine(sine_args) = &cli.command {
            assert!(sine_args.noise.noise);
            assert_eq!(sine_args.noise.noise_mean, TrueDuration::from_nanos(2500));
            assert_eq!(
                sine_args.noise.noise_std_dev,
                Some(TrueDuration::from_nanos(7500))
            );
            assert_eq!(sine_args.noise.noise_step, TrueDuration::from_millis(20));
            assert_eq!(sine_args.noise.noise_seed, Some(456));
        } else {
            panic!("Wrong command parsed")
        }
    }

    /// Tests default values for sine oscillator configuration
    #[test]
    fn sine_default_values() {
        let args = vec!["test", "sine"];
        let cli = Cli::try_parse_from(args).unwrap();

        if let Commands::Sine(sine_args) = &cli.command {
            assert_eq!(sine_args.period, TrueDuration::from_minutes(5));
            assert_eq!(sine_args.amplitude, TrueDuration::from_micros(40));
            assert_eq!(sine_args.sample_period, TrueDuration::from_secs(1));
            assert_eq!(sine_args.common.clock_frequency, Frequency::from_ghz(1.0));
            assert_eq!(sine_args.common.duration, TrueDuration::from_minutes(5));
            assert_eq!(sine_args.common.tsc_timestamp_start, TscCount::new(0));
            assert_eq!(sine_args.common.output, None);

            // Check default noise parameters
            assert!(!sine_args.noise.noise);
            assert_eq!(sine_args.noise.noise_mean, TrueDuration::from_secs(0));
            assert_eq!(sine_args.noise.noise_std_dev, None);
            assert_eq!(sine_args.noise.noise_step, TrueDuration::from_secs(1));
            assert_eq!(sine_args.noise.noise_seed, None);
        } else {
            panic!("Wrong command parsed")
        }
    }
}
