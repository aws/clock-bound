// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use clock_bound_c::ClockBoundClient;
use std::env;
use std::thread::sleep;
use std::time::Duration;

const ONE_SECOND_IN_NANOSECONDS: u64 = 1000000000;

fn main() {
    let args: Vec<String> = env::args().collect();
    let clock_bound_d_socket = &args[1];

    let client =
        match ClockBoundClient::new_with_path(std::path::PathBuf::from(clock_bound_d_socket)) {
            Ok(client) => client,
            Err(e) => {
                println!("Could not create client: {}", e);
                return;
            }
        };

    // Get the current time in nanoseconds since the Unix Epoch
    let timestamp = Utc::now().timestamp_nanos() as u64;

    // Test with a timestamp 1 second in the future
    let timestamp = timestamp + ONE_SECOND_IN_NANOSECONDS;

    // Checks if a point in time is after the current time's error bounds
    let response = match client.after(timestamp) {
        Ok(response) => response,
        Err(e) => {
            println!("Couldn't complete after request: {}", e);
            return;
        }
    };

    // A clock synchronized via NTP should generally not be a second off. One second past the
    // current time should generally be after the latest error bound and should return true.
    if response.after == false {
        println!(
            "{} nanoseconds since the Unix Epoch is not after the current time's error bounds.",
            timestamp
        )
    } else if response.after == true {
        println!(
            "{} nanoseconds since the Unix Epoch is after the current time's error bounds.",
            timestamp
        )
    }

    println!("Waiting 2 seconds...");

    // Checking again after the timestamp has passed should no longer be after the latest error
    // bound and return false.
    sleep(Duration::from_secs(2));

    // Checks if a point in time is after the current time's error bounds
    let response = match client.after(timestamp) {
        Ok(response) => response,
        Err(e) => {
            println!("Couldn't complete after request: {}", e);
            return;
        }
    };

    if response.after == false {
        println!(
            "{} nanoseconds since the Unix Epoch is not after the current time's error bounds.",
            timestamp
        )
    } else if response.after == true {
        println!(
            "{} nanoseconds since the Unix Epoch is after the current time's error bounds.",
            timestamp
        )
    }
}
