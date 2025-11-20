//! A program to read the ClockBound SHM segment's earliest and latest bounds.

use clap::Parser;
use clock_bound_client_generic::{
    ClockBoundClient, ClockBoundClientClock, ClockBoundClientVersion,
};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[clap(value_enum, long)]
    client_version: ClockBoundClientVersion,
}

fn main() {
    tracing_subscriber::fmt().json().init();
    let args = Args::parse();

    let mut client = ClockBoundClient::new(args.client_version);
    match client.get_time_and_bound_and_status() {
        Ok(data) => println!("{}", serde_json::to_string(&data).unwrap()),
        Err(e) => tracing::error!("Failed to retrieve clock bounds: {e:?}"),
    }
}
