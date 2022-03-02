// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
use chrono::prelude::DateTime;
use chrono::Utc;
use clock_bound_c::ClockBoundClient;
use std::env;

fn foo() -> fn() -> i32 {
    move || return 1
}

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

    let (response, _result) = match client.timing(foo()) {
        Ok((response, result)) => (response, result),
        Err((e, _res)) => {
            println!("Could not complete timing request: {}", e);
            return;
        }
    };

    let datetime_earliest: DateTime<Utc> = response.earliest_start.into();
    let datetime_latest: DateTime<Utc> = response.latest_finish.into();
    let datetime_str_earliest = datetime_earliest.format("%Y-%m-%d %H:%M:%S.%f").to_string();
    let datetime_str_latest = datetime_latest.format("%Y-%m-%d %H:%M:%S.%f").to_string();

    println!(
        "Earliest start time for the timing request: {:?}", datetime_str_earliest
    );
    println!(
        "Latest finish time for the timing request: {:?}", datetime_str_latest
    );
    println!(
        "Minimum execution duration of timing request: {:?}", response.min_execution_time
    );
    println!(
        "Maximum execution duration of timing request: {:?}", response.max_execution_time
    )
}
