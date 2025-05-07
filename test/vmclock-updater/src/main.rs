use std::path::Path;
use std::process;
use std::str::FromStr;

use clap::Parser;

use clock_bound_vmclock::shm::{VMClockClockStatus, VMClockShmBody, VMCLOCK_SHM_DEFAULT_PATH};
use clock_bound_vmclock::shm_writer::{VMClockShmWrite, VMClockShmWriter};

/// CLI arguments are the possible field values that can be set in the VMClock shared memory segment.
#[derive(Parser, Debug)]
#[command(author, name = "vmclock-updater", version, about, long_about = None)]
struct Cli {
    /// Disruption Marker.
    ///
    /// This value is incremented (by an unspecified delta) each time the clock is disrupted.
    /// This count value is specific to a particular VM/EC2 instance.
    #[arg(long)]
    disruption_marker: Option<u64>,

    /// Flags.
    #[arg(long)]
    counter_value: Option<u64>,

    /// Clock Status.
    ///
    /// The clock status indicates whether the clock is synchronized,
    /// free-running, etc.
    ///
    /// Maps to enum VMClockClockStatus.
    #[arg(long)]
    clock_status: Option<String>,

    /// Leap Second smearing hint.
    #[arg(long)]
    leap_second_smearing_hint: Option<u8>,

    /// Offset between TAI and UTC, in seconds.
    #[arg(long)]
    tai_offset_sec: Option<i16>,

    /// Leap indicator.
    #[arg(long)]
    leap_indicator: Option<u8>,

    /// Counter period shift.
    #[arg(long)]
    counter_period_shift: Option<u8>,

    /// Counter period.
    #[arg(long)]
    counter_period_frac_sec: Option<u64>,

    /// Counter period estimated error rate.
    #[arg(long)]
    counter_period_esterror_rate_frac_sec: Option<u64>,

    /// Counter period maximum error rate.
    #[arg(long)]
    counter_period_maxerror_rate_frac_sec: Option<u64>,

    /// Time: Seconds since time_type epoch.
    #[arg(long)]
    time_sec: Option<u64>,

    /// Time: Fractional seconds, in units of 1 / (2 ^ 64) of a second.
    #[arg(long)]
    time_frac_sec: Option<u64>,

    /// Time: Estimated error.
    #[arg(long)]
    time_esterror_nanosec: Option<u64>,

    /// Time: Maximum error.
    #[arg(long)]
    time_maxerror_nanosec: Option<u64>,
}

fn main() {
    let args = Cli::parse();
    let vmclock_shm_path = Path::new(VMCLOCK_SHM_DEFAULT_PATH);

    let mut vmclock_shm_writer = match VMClockShmWriter::new(vmclock_shm_path) {
        Ok(writer) => writer,
        Err(e) => {
            eprintln!("VMClockShmWriter::new() failed. {:?}", &e);
            process::exit(1);
        }
    };

    // Create an initial default VMClockShmBody.
    let mut vmclock_shm_body = VMClockShmBody::default();

    // Override the VMClockShmBody with any values provided by CLI arguments.
    if let Some(disruption_marker) = args.disruption_marker {
        vmclock_shm_body.disruption_marker = disruption_marker;
    }
    if let Some(counter_value) = args.counter_value {
        vmclock_shm_body.counter_value = counter_value;
    }
    if let Some(clock_status_string) = args.clock_status {
        let vmclock_clock_status = match VMClockClockStatus::from_str(clock_status_string.as_str())
        {
            Ok(vmclock_clock_status) => vmclock_clock_status,
            Err(e) => {
                eprintln!(
                    "Failed to convert clock_status argument '{:?}' to a VMClockClockStatus. {:?}",
                    clock_status_string, &e
                );
                process::exit(1);
            }
        };
        vmclock_shm_body.clock_status = vmclock_clock_status;
    }
    if let Some(leap_second_smearing_hint) = args.leap_second_smearing_hint {
        vmclock_shm_body.leap_second_smearing_hint = leap_second_smearing_hint;
    }
    if let Some(tai_offset_sec) = args.tai_offset_sec {
        vmclock_shm_body.tai_offset_sec = tai_offset_sec;
    }
    if let Some(leap_indicator) = args.leap_indicator {
        vmclock_shm_body.leap_indicator = leap_indicator;
    }
    if let Some(counter_period_shift) = args.counter_period_shift {
        vmclock_shm_body.counter_period_shift = counter_period_shift;
    }
    if let Some(counter_period_frac_sec) = args.counter_period_frac_sec {
        vmclock_shm_body.counter_period_frac_sec = counter_period_frac_sec;
    }
    if let Some(counter_period_esterror_rate_frac_sec) = args.counter_period_esterror_rate_frac_sec
    {
        vmclock_shm_body.counter_period_esterror_rate_frac_sec =
            counter_period_esterror_rate_frac_sec;
    }
    if let Some(counter_period_maxerror_rate_frac_sec) = args.counter_period_maxerror_rate_frac_sec
    {
        vmclock_shm_body.counter_period_maxerror_rate_frac_sec =
            counter_period_maxerror_rate_frac_sec;
    }
    if let Some(time_sec) = args.time_sec {
        vmclock_shm_body.time_sec = time_sec;
    }
    if let Some(time_frac_sec) = args.time_frac_sec {
        vmclock_shm_body.time_frac_sec = time_frac_sec;
    }
    if let Some(time_esterror_nanosec) = args.time_esterror_nanosec {
        vmclock_shm_body.time_esterror_nanosec = time_esterror_nanosec;
    }
    if let Some(time_maxerror_nanosec) = args.time_maxerror_nanosec {
        vmclock_shm_body.time_maxerror_nanosec = time_maxerror_nanosec;
    }

    // Write to the VMClock shared memory segment.
    vmclock_shm_writer.write(&vmclock_shm_body);
    println!("Successfully wrote the following VMClockShmBody to the VMClock shared memory segment: {:?}", vmclock_shm_body);
}
