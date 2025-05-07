use clock_bound_client::{
    ClockBoundClient, ClockBoundError, ClockStatus, CLOCKBOUND_SHM_DEFAULT_PATH,
    VMCLOCK_SHM_DEFAULT_PATH,
};
use std::process;
use std::thread;
use std::time::Duration;

fn main() {
    let mut clockbound = match ClockBoundClient::new_with_paths(
        CLOCKBOUND_SHM_DEFAULT_PATH,
        VMCLOCK_SHM_DEFAULT_PATH,
    ) {
        Ok(c) => c,
        Err(e) => {
            print_error("ClockBoundClient::new() failed", &e);
            process::exit(1);
        }
    };

    // Run forever.
    // Type Ctrl-C to send a SIGINT and quit the program.
    loop {
        let now_result = match clockbound.now() {
            Ok(result) => result,
            Err(e) => {
                print_error("ClockBoundClient::now() failed", &e);
                process::exit(1);
            }
        };

        println!("When clockbound_now was called true time was somewhere within {}.{:0>9} and {}.{:0>9} seconds since Jan 1 1970. The clock status is {:?}.",
                 &now_result.earliest.tv_sec(), &now_result.earliest.tv_nsec(),
                 &now_result.latest.tv_sec(), &now_result.latest.tv_nsec(),
                 format_clock_status(&now_result.clock_status));

        thread::sleep(Duration::from_millis(1000));
    }
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
}
