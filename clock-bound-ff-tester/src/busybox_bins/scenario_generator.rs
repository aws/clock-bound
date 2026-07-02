//! Scenario Generator CLI Tool
//!
//! This tool generates NTP scenarios by combining oscillator models with
//! network configuration. It reads a serialized oscillator file, adds NTP
//! source parameters, and outputs a complete scenario.

use std::{fs::File, io::BufReader, path::PathBuf};

use crate::events::Scenario as TesterScenario;
use crate::simulation::{
    delay::{Delay, DelayRng, TimeUnit},
    generator::GeneratorExt,
    ntp::{VariableNetworkDelayGenerator, VariableRoundTripDelays},
    oscillator::{FullModel, Oscillator},
    phc,
    stats::{self, DiracDistribution, GammaDistribution},
    vmclock,
};
use crate::time::{EstimateDuration, Period, TrueDuration};
use anyhow::{Context, Ok};
use clap::{Parser, Subcommand};
use rand::rngs::OsRng;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DelayArgs {
    Constant(TrueDuration),
    GammaDistribution(stats::GammaDistribution),
    Period(Period),
}

impl DelayArgs {
    fn to_boxed_delay(self) -> Box<dyn DelayRng + 'static> {
        match self {
            DelayArgs::Constant(v) => Box::new(Delay::new(
                #[allow(clippy::cast_precision_loss)]
                DiracDistribution::new(v.as_nanos() as f64).unwrap(),
                TimeUnit::Nanos,
            )),
            DelayArgs::GammaDistribution(d) => Box::new(Delay::new(d, TimeUnit::Secs)),
            DelayArgs::Period(p) => Box::new(Delay::new(
                DiracDistribution::new(p.get()).unwrap(),
                TimeUnit::Secs,
            )),
        }
    }
}

impl FromStr for DelayArgs {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.starts_with("Constant") {
            // Strips Constant prefix,
            // strips parenthesis then parses the
            // TrueDuration from the given string.
            let s = s.strip_prefix("Constant").unwrap();
            let s = s
                .strip_prefix('(')
                .and_then(|s| s.strip_suffix(')'))
                .ok_or("Did not find parenthesis.")?;

            let rv = TrueDuration::from_str(s).unwrap();
            return Result::Ok(DelayArgs::Constant(rv));
        } else if s.starts_with("Gamma") {
            // Strips Gamma prefix,
            // parses the Gamma distribution parameters from the given string.
            let s = s.strip_prefix("Gamma").unwrap();
            let d = GammaDistribution::from_str(s).unwrap();
            return Result::Ok(DelayArgs::GammaDistribution(d));
        } else if s.starts_with("Period") {
            // Strips Period prefix,
            // parses the Period from the given string.
            let s = s.strip_prefix("Period").unwrap();
            let s = s
                .strip_prefix('(')
                .and_then(|s| s.strip_suffix(')'))
                .ok_or("Did not find parenthesis.")?;
            let p = Period::from_str(s).unwrap();
            return Result::Ok(DelayArgs::Period(p));
        }
        Err(format!("Unexpected prefix. Input: {s}"))
    }
}

/// Command-line interface for the scenario generator
#[derive(Clone, Debug, Parser)]
pub struct Cli {
    /// Path to input oscillator JSON file
    #[arg(long)]
    pub oscillator_file: PathBuf,

    /// Optional output file name, defaults to stdout
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

/// Available generator types
#[derive(Clone, Debug, Subcommand)]
pub enum Commands {
    /// Generate an NTP scenario
    Ntp(Ntp),
    /// Generate a PHC scenario
    Phc(Phc),
    /// Generate a VMClock scenario
    VMClock(VMClock),
}

/// Configuration for NTP scenario generator
#[derive(Debug, Clone, Parser)]
pub struct Ntp {
    /// The forward network delay of NTP packets.
    ///
    /// Inputs can be constant or from a distribution.
    ///
    /// To use the gamma distribution Include the shape, rate and loc parameters
    /// in that order in the following format.
    /// Gamma{`shape`,`rate`,`loc`}
    ///
    /// When using a distribution the associated unit are microseconds.
    #[arg(long, default_value = "Constant(25microseconds)")]
    pub forward_network: DelayArgs,

    /// The backward network delay of NTP packets.
    ///
    /// Inputs can be constant or from a distribution.
    ///
    /// To use the gamma distribution Include the shape, rate and loc parameters
    /// in that order in the following format.
    /// Gamma{`shape`,`rate`,`loc`}
    ///
    /// When using a distribution the associated unit are microseconds.
    #[arg(long, default_value = "Constant(30microseconds)")]
    pub backward_network: DelayArgs,

    /// The NTP server processing time.
    ///
    /// Inputs can be constant or from a distribution.
    ///
    /// To use the gamma distribution Include the shape, rate and loc parameters
    /// in that order in the following format.
    /// Gamma{`shape`,`rate`,`loc`}
    ///
    /// When using a distribution the associated unit are microseconds.
    #[arg(long, default_value = "Constant(15microseconds)")]
    pub server_delay: DelayArgs,

    /// How often the local client polls the NTP server
    #[arg(long, default_value = "16s")]
    pub poll_period: EstimateDuration,

    /// Identifier for this NTP source
    #[arg(long, required = true)]
    pub id: String,
}

/// Configuration for the PHC scenario generator
#[derive(Debug, Clone, Parser)]
pub struct Phc {
    /// The forward network delay of the PHC (`ref_clock`) read.
    ///
    /// Inputs can be constant or from a distribution.
    ///
    /// To use the gamma distribution Include the shape, rate and loc parameters
    /// in that order in the following format.
    /// Gamma{`shape`,`rate`,`loc`}
    ///
    /// When using a distribution the associated unit are microseconds.
    #[arg(long, default_value = "Constant(5microseconds)")]
    pub forward_network: DelayArgs,

    /// The backward network delay of the PHC (`ref_clock`) read.
    ///
    /// Inputs can be constant or from a distribution.
    ///
    /// To use the gamma distribution Include the shape, rate and loc parameters
    /// in that order in the following format.
    /// Gamma{`shape`,`rate`,`loc`}
    ///
    /// When using a distribution the associated unit are microseconds.
    #[arg(long, default_value = "Constant(8microseconds)")]
    pub backward_network: DelayArgs,

    /// The clock error bound of the PHC value
    /// Inputs can be constant or from a distribution
    #[arg(long, alias = "ceb")]
    pub clock_error_bound: Option<DelayArgs>,

    /// How often the local client polls the NTP server
    #[arg(long, default_value = "50s")]
    pub poll_period: EstimateDuration,

    /// Identifier for this PHC source
    #[arg(long, required = true)]
    pub id: String,
}

/// Configuration for generating a single VMClock generator
#[derive(Debug, Clone, Parser)]
pub struct VMClock {
    /// How often the VMClock gets updated
    #[arg(long, default_value = "50s")]
    pub update_period: EstimateDuration,

    /// The period error distribution
    #[arg(long, alias = "period-error")]
    pub period_max_error: Option<DelayArgs>,

    /// The lag value of the VMClock time
    #[arg(long, default_value = "Constant(1milliseconds)")]
    pub vmclock_time_lag: DelayArgs,

    /// Identifier for this VMClock source
    #[arg(long)]
    pub id: String,
}

/// Main function of the scenario generator tool
///
/// This function:
/// 1. Parses command-line arguments
/// 2. Deserializes an oscillator model from the specified file
/// 3. Creates a full model from the oscillator
/// 4. Sets up an generator with the specified parameters
/// 5. Generates a scenario with events
/// 6. Serializes the scenario to a file or stdout
pub fn main(cli: Cli) -> anyhow::Result<()> {
    let oscillator = deserialize_oscillator(&cli.oscillator_file).with_context(|| {
        format!(
            "Failed to load oscillator from file '{}'",
            cli.oscillator_file.display()
        )
    })?;
    let full_model = FullModel::calculate_from_oscillator(oscillator);

    let scenario = match cli.command {
        Commands::Ntp(ntp) => {
            let mut generator = create_ntp_generator(
                ntp.forward_network.to_boxed_delay(),
                ntp.server_delay.to_boxed_delay(),
                ntp.backward_network.to_boxed_delay(),
                ntp.id,
                ntp.poll_period,
                &full_model,
            );
            generator.create_scenario(full_model)
        }
        Commands::Phc(phc) => {
            let delays = phc::round_trip_delays::VariableRoundTripDelays::builder()
                .forward_network(phc.forward_network.to_boxed_delay())
                .backward_network(phc.backward_network.to_boxed_delay())
                .build();
            let mut generator = phc::variable_delay_source::Generator::builder()
                .poll_period(phc.poll_period)
                .id(phc.id)
                .oscillator(&full_model)
                .network_delays(delays)
                .maybe_clock_error_bounds(phc.clock_error_bound.map(DelayArgs::to_boxed_delay))
                .build();

            generator.create_scenario(full_model)
        }
        Commands::VMClock(vmc) => {
            let props = vmclock::Props {
                update_period: vmc.update_period,
                clock_period_max_error: vmc.period_max_error.map(DelayArgs::to_boxed_delay),
                vmclock_time_lag: vmc.vmclock_time_lag.to_boxed_delay(),
            };

            let mut generator = vmclock::Generator::builder()
                .props(props)
                .id(vmc.id)
                .oscillator(&full_model)
                .rng(Box::new(OsRng))
                .build();

            generator.create_scenario(full_model)
        }
    };

    serialize_scenario(cli.output.as_ref(), &scenario).with_context(|| {
        format!(
            "Failed to serialize scenario{}",
            cli.output.as_ref().map_or_else(
                || " to stdout".to_string(),
                |p| format!(" to '{}'", p.display())
            )
        )
    })?;

    Ok(())
}

fn deserialize_oscillator(path: &PathBuf) -> Result<Oscillator, anyhow::Error> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open oscillator file '{}'", path.display()))?;
    let reader = BufReader::new(file);
    let inner: crate::events::v1::Oscillator = serde_json::from_reader(reader)
        .with_context(|| format!("Failed to parse oscillator JSON from '{}'", path.display()))?;
    let oscillator = Oscillator::from(inner);
    Ok(oscillator)
}

fn create_ntp_generator(
    forward_network: Box<dyn DelayRng>,
    backward_network: Box<dyn DelayRng>,
    server_delay: Box<dyn DelayRng>,
    id: String,
    poll_period: EstimateDuration,
    oscillator: &FullModel,
) -> VariableNetworkDelayGenerator {
    let round_trip_model = VariableRoundTripDelays::builder()
        .forward_network(forward_network)
        .backward_network(backward_network)
        .server(server_delay)
        .build();

    VariableNetworkDelayGenerator::builder()
        .id(id)
        .poll_period(poll_period)
        .network_delays(round_trip_model)
        .oscillator(oscillator)
        .build()
}

fn serialize_scenario(
    path: Option<&PathBuf>,
    scenario: &TesterScenario,
) -> Result<(), anyhow::Error> {
    if let Some(output) = path.as_ref() {
        let mut file = File::create(output)
            .with_context(|| format!("Failed to create output file '{}'", output.display()))?;
        crate::events::serialize_to_writer_pretty(scenario, &mut file).with_context(|| {
            format!(
                "Failed to serialize scenario to file '{}'",
                output.display()
            )
        })?;
        println!("Wrote scenario to {}", output.display());
    } else {
        let mut out = std::io::stdout().lock();
        crate::events::serialize_to_writer_pretty(scenario, &mut out)
            .context("Failed to serialize scenario to stdout")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{Frequency, TscCount};
    use serde_json::{Value, json};
    use std::io::{Read, Write};
    use tempfile::NamedTempFile;

    /// Creates a sine oscillator file for testing
    /// with parameters similar to the example output
    fn create_sine_oscillator_file() -> anyhow::Result<NamedTempFile> {
        // This matches the sine oscillator output with 20s duration and 1s sample period
        let oscillator_json = json!({
            "system_start_time": 1_748_583_788_901_252_492_i64,
            "tsc_timestamp_start": 0,
            "clock_frequency": 1_000_000_000.0_f64,
            "oscillator_offsets": {
                "inner": {
                    "indices": [
                        0_i64, 1_000_000_000_i64, 2_000_000_000_i64, 3_000_000_000_i64, 4_000_000_000_i64,
                        5_000_000_000_i64, 6_000_000_000_i64, 7_000_000_000_i64, 8_000_000_000_i64, 9_000_000_000_i64,
                        10_000_000_000_i64, 11_000_000_000_i64, 12_000_000_000_i64, 13_000_000_000_i64, 14_000_000_000_i64,
                        15_000_000_000_i64, 16_000_000_000_i64, 17_000_000_000_i64, 18_000_000_000_i64, 19_000_000_000_i64
                    ],
                    "data": [
                        0, 837, 1675, 2511, 3347,
                        4181, 5013, 5843, 6670, 7495,
                        8316, 9134, 9947, 10756, 11561,
                        12360, 13154, 13942, 14724, 15500
                    ]
                }
            }
        });

        let mut temp_file = NamedTempFile::new()?;
        write!(temp_file, "{oscillator_json}")?;
        Ok(temp_file)
    }

    #[test]
    fn deserialize_oscillator_basic() -> anyhow::Result<()> {
        let oscillator_file = create_sine_oscillator_file()?;
        let oscillator = deserialize_oscillator(&oscillator_file.path().to_path_buf())?;

        assert_eq!(oscillator.clock_frequency(), Frequency::from_ghz(1.0));
        assert_eq!(oscillator.tsc_timestamp_start(), TscCount::new(0));

        // Verify the offsets are correctly loaded - 20 samples matching example
        let offsets = oscillator.offset_series().as_ref();
        assert_eq!(offsets.len(), 20);
        assert_eq!(offsets.indices()[0].as_nanos(), 0);
        assert_eq!(offsets.indices()[19].as_nanos(), 19_000_000_000);

        // Check first and last
        assert_eq!(offsets.data()[0].as_nanos(), 0);
        assert_eq!(offsets.data()[19].as_nanos(), 15500);

        Ok(())
    }

    #[test]
    fn scenario_generator_basic_functionality() {
        // Create a scenario using the same parameters as in sample
        let oscillator_file = create_sine_oscillator_file().unwrap();
        let oscillator = deserialize_oscillator(&oscillator_file.path().to_path_buf()).unwrap();
        let full_model = FullModel::calculate_from_oscillator(oscillator);
        let mut generator = create_ntp_generator(
            Box::new(Delay::new(
                DiracDistribution::new(25.0).unwrap(),
                TimeUnit::Micros,
            )),
            Box::new(Delay::new(
                DiracDistribution::new(15.0).unwrap(),
                TimeUnit::Micros,
            )),
            Box::new(Delay::new(
                DiracDistribution::new(30.0).unwrap(),
                TimeUnit::Micros,
            )),
            "sine".to_string(),
            EstimateDuration::from_secs(16),
            &full_model,
        );
        let scenario = generator.create_scenario(full_model);

        // Test file serialization
        let temp_file = NamedTempFile::new().unwrap();
        let path = Some(temp_file.path().to_path_buf());

        serialize_scenario(path.as_ref(), &scenario).unwrap();

        // Read and parse the serialized JSON to verify structure
        let mut content = String::new();
        let mut file = File::open(temp_file.path()).unwrap();
        _ = file.read_to_string(&mut content).unwrap();
        let json_value: Value = serde_json::from_str(&content).unwrap();

        // Verify the correct top-level structure with "V0" key
        assert!(json_value.get("V1").is_some());

        // Check the events array
        let events = &json_value["V1"]["events"];
        assert!(events.is_array());
        assert!(!events.as_array().unwrap().is_empty());

        // Verify first event structure
        let first_event = &events[0];
        assert!(first_event["variants"]["Ntp"].is_object());
        assert!(first_event["client_tsc_post_time"].is_number());
        assert!(first_event["client_tsc_pre_time"].is_number());

        // Check that NTP event fields exist
        let ntp_event = &first_event["variants"]["Ntp"];
        assert!(ntp_event["server_system_recv_time"].is_number());
        assert!(ntp_event["server_system_send_time"].is_number());
        assert_eq!(ntp_event["root_delay"], 0);
        assert_eq!(ntp_event["root_dispersion"], 0);
        assert_eq!(ntp_event["source_id"], "sine");

        // Check oscillator scenario structure
        let oscillator = &json_value["V1"]["oscillator"];
        assert!(oscillator["system_start_time"].is_number());
        assert_eq!(oscillator["tsc_timestamp_start"], 0);
        assert_eq!(oscillator["clock_frequency"], 1_000_000_000.0);

        // Verify oscillator offsets
        let offsets = &oscillator["oscillator_offsets"]["inner"];
        assert!(offsets["indices"].is_array());
        assert!(offsets["data"].is_array());
        assert_eq!(offsets["indices"].as_array().unwrap().len(), 20);
        assert_eq!(offsets["data"].as_array().unwrap().len(), 20);

        // Metadata
        assert!(json_value["V1"]["metadata"].is_object());
    }

    #[test]
    fn cli_parsing() {
        // with minimal arguments
        let args = [
            "scenario_generator",
            "--oscillator-file",
            "oscillator.json",
            "ntp",
            "--id",
            "test-source",
        ];

        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.oscillator_file, PathBuf::from("oscillator.json"));
        assert_eq!(cli.output, None);
        let Commands::Ntp(cli) = cli.command else {
            panic!("Expected NTP command");
        };
        assert_eq!(cli.id, "test-source");
        assert_eq!(
            cli.forward_network,
            DelayArgs::Constant(TrueDuration::from_micros(25))
        );
        assert_eq!(cli.poll_period, EstimateDuration::from_secs(16));

        // with all arguments
        let args = [
            "scenario_generator",
            "--oscillator-file",
            "oscillator.json",
            "--output",
            "output.json",
            "ntp",
            "--forward-network",
            "Constant(40microseconds)",
            "--backward-network",
            "Constant(50microseconds)",
            "--server-delay",
            "Constant(10microseconds)",
            "--poll-period",
            "8s",
            "--id",
            "custom-source",
        ];

        let cli = Cli::try_parse_from(args).unwrap();
        assert_eq!(cli.oscillator_file, PathBuf::from("oscillator.json"));
        assert_eq!(cli.output, Some(PathBuf::from("output.json")));
        let Commands::Ntp(cli) = cli.command else {
            panic!("Expected NTP command");
        };
        assert_eq!(
            cli.forward_network,
            DelayArgs::Constant(TrueDuration::from_micros(40))
        );
        assert_eq!(
            cli.backward_network,
            DelayArgs::Constant(TrueDuration::from_micros(50))
        );
        assert_eq!(
            cli.server_delay,
            DelayArgs::Constant(TrueDuration::from_micros(10))
        );
        assert_eq!(cli.poll_period, EstimateDuration::from_secs(8));
        assert_eq!(cli.id, "custom-source");
    }

    #[test]
    fn cli_required_arguments() {
        // Missing required oscillator file
        let args = ["scenario_generator", "--id", "test-source"];

        let result = Cli::try_parse_from(args);
        _ = result.unwrap_err();

        // Missing required id
        let args = ["scenario_generator", "--oscillator-file", "oscillator.json"];

        let result = Cli::try_parse_from(args);
        _ = result.unwrap_err();
    }
}
