// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
use chrono::prelude::DateTime;
use chrono::Utc;
use clock_bound_c::ClockBoundClient;
use std::env;
use std::time::{Duration, UNIX_EPOCH};

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

    let response = match client.now() {
        Ok(response) => response,
        Err(e) => {
            println!("Could not complete now request: {}", e);
            return;
        }
    };

    let earliest_d = UNIX_EPOCH + Duration::from_nanos(response.bound.earliest);
    let latest_d = UNIX_EPOCH + Duration::from_nanos(response.bound.latest);
    let timestamp_d = UNIX_EPOCH + Duration::from_nanos(response.timestamp);
    let datetime_earliest = DateTime::<Utc>::from(earliest_d);
    let datetime_latest = DateTime::<Utc>::from(latest_d);
    let datetime_timestamp = DateTime::<Utc>::from(timestamp_d);
    let datetime_str_earliest = datetime_earliest.format("%Y-%m-%d %H:%M:%S.%f").to_string();
    let datetime_str_latest = datetime_latest.format("%Y-%m-%d %H:%M:%S.%f").to_string();
    let datetime_str_timestamp = datetime_timestamp
        .format("%Y-%m-%d %H:%M:%S.%f")
        .to_string();

    println!(
        "The UTC timestamp {} has the following error bounds.",
        datetime_str_timestamp
    );
    println!(
        "In nanoseconds since the Unix epoch: ({:?},{:?})",
        response.bound.earliest, response.bound.latest
    );
    println!(
        "In UTC in date/time format: ({}, {})",
        datetime_str_earliest, datetime_str_latest
    );
}
