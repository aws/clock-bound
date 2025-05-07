use clock_bound_client::{
    ClockBoundClient, ClockBoundError, ClockStatus, CLOCKBOUND_SHM_DEFAULT_PATH,
    VMCLOCK_SHM_DEFAULT_PATH,
};
use nix::sys::time::TimeSpec;
use std::process;

fn main() {
    let mut clockbound = match ClockBoundClient::new_with_paths(
        CLOCKBOUND_SHM_DEFAULT_PATH,
        VMCLOCK_SHM_DEFAULT_PATH,
    ) {
        Ok(c) => c,
        Err(e) => {
            print_error("ClockBoundClient::new_with_paths() failed", &e);
            process::exit(1);
        }
    };

    let now_result_first = match clockbound.now() {
        Ok(result) => result,
        Err(e) => {
            print_error("ClockBoundClient::now() failed", &e);
            process::exit(1);
        }
    };

    println!("When clockbound_now was called true time was somewhere within {}.{:0>9} and {}.{:0>9} seconds since Jan 1 1970. The clock status is {:?}.",
             &now_result_first.earliest.tv_sec(), &now_result_first.earliest.tv_nsec(),
             &now_result_first.latest.tv_sec(), &now_result_first.latest.tv_nsec(),
             format_clock_status(&now_result_first.clock_status));

    // Very naive performance benchmark.
    let call_count = 100_000_000;
    let mut now_result_last = now_result_first.clone();
    for _ in 0..call_count {
        now_result_last = match clockbound.now() {
            Ok(result) => result,
            Err(e) => {
                print_error("ClockBoundClient::now() failed", &e);
                process::exit(1);
            }
        };
    }

    let duration_seconds =
        calculate_duration_seconds(&now_result_first.earliest, &now_result_last.earliest);

    println!(
        "It took {duration_seconds} seconds to call clock bound {call_count} times ({} tps))",
        (call_count as f64 / duration_seconds) as i32
    );
}

fn print_error(detail: &str, error: &ClockBoundError) {
    eprintln!("{detail} {:?}", error);
}

fn format_clock_status(clock_status: &ClockStatus) -> &str {
    match clock_status {
        ClockStatus::Unknown => "Unknown",
        ClockStatus::Synchronized => "Synchronized",
        ClockStatus::FreeRunning => "FreeRunning",
        ClockStatus::Disrupted => "Disrupted",
    }
}

// Helper function to calculate the time interval between two timestamps held in TimeSpecs.
fn calculate_duration_seconds(start: &TimeSpec, end: &TimeSpec) -> f64 {
    let mut secs: i64 = end.tv_sec() - start.tv_sec();
    let mut nsecs: i64 = end.tv_nsec() - start.tv_nsec();

    if nsecs < 0 {
        nsecs += 1_000_000_000;
        secs -= 1;
    }

    (secs as f64) + (nsecs as f64 / 1_000_000_000_f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clock_bound_client::ClockBoundErrorKind;
    use errno::Errno;

    #[test]
    fn test_print_error() {
        let details = "Some details about the error";
        let clock_bound_error_details = "ClockBound daemon not running";
        let errno_number = 9999;

        let clock_bound_error = ClockBoundError {
            kind: ClockBoundErrorKind::SegmentNotInitialized,
            errno: Errno(errno_number),
            detail: clock_bound_error_details.to_string(),
        };

        // Verify it runs and does not panic.
        print_error(&details, &clock_bound_error);
        assert!(true);
    }

    #[test]
    fn test_format_clock_status() {
        let formatted_str = format_clock_status(&ClockStatus::Unknown);
        assert_eq!(formatted_str, "Unknown");

        let formatted_str = format_clock_status(&ClockStatus::Synchronized);
        assert_eq!(formatted_str, "Synchronized");

        let formatted_str = format_clock_status(&ClockStatus::FreeRunning);
        assert_eq!(formatted_str, "FreeRunning");

        let formatted_str = format_clock_status(&ClockStatus::Disrupted);
        assert_eq!(formatted_str, "Disrupted");
    }

    #[test]
    fn test_calculate_duration_seconds() {
        // No wrap of nanoseconds.
        let start = TimeSpec::new(100, 5);
        let end = TimeSpec::new(100, 6);
        let result: f64 = calculate_duration_seconds(&start, &end);
        assert_eq!(result, 0.000000001_f64);

        // Wrap of nanoseconds.
        let start = TimeSpec::new(100, 5);
        let end = TimeSpec::new(101, 0);
        let result: f64 = calculate_duration_seconds(&start, &end);
        assert_eq!(result, 0.999999995_f64);

        // Same timestamp.
        let start = TimeSpec::new(200, 0);
        let end = TimeSpec::new(200, 0);
        let result: f64 = calculate_duration_seconds(&start, &end);
        assert_eq!(result, 0.0_f64);

        // Negative duration seconds.
        let start = TimeSpec::new(200, 0);
        let end = TimeSpec::new(199, 0);
        let result: f64 = calculate_duration_seconds(&start, &end);
        assert_eq!(result, -1.0_f64);
    }
}
