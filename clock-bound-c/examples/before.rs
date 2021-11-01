// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
use chrono::Utc;
use clock_bound_c::ClockBoundClient;
use std::env;
use std::thread::sleep;
use std::time::Duration;

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

    // Checks if a point in time is before the current time's error bounds
    let response = match client.before(timestamp) {
        Ok(response) => response,
        Err(e) => {
            println!("Couldn't complete before request: {}", e);
            return;
        }
    };

    // With a before request done using the current time for comparison, it is likely that the
    // request is processed faster than the local Clock Error Bound. This means that generally the
    // current time will not be before the earliest bound and therefore return false.
    if response.before == false {
        println!(
            "{} nanoseconds since the Unix Epoch is not before the current time's error bounds.",
            timestamp
        )
    } else if response.before == true {
        println!(
            "{} nanoseconds since the Unix Epoch is before the current time's error bounds.",
            timestamp
        )
    }

    println!("Waiting 1 second...");

    // Checking again after a brief period of time (a pessimistic 1 second for this example's sake)
    // the timestamp should be before the earliest error bound and return true.
    sleep(Duration::from_secs(1));

    // Checks if a point in time is before the current time's error bounds
    let response = match client.before(timestamp) {
        Ok(response) => response,
        Err(e) => {
            println!("Couldn't complete before request: {}", e);
            return;
        }
    };

    if response.before == false {
        println!(
            "{} nanoseconds since the Unix Epoch is not before the current time's error bounds.",
            timestamp
        )
    } else if response.before == true {
        println!(
            "{} nanoseconds since the Unix Epoch is before the current time's error bounds.",
            timestamp
        )
    }
}
