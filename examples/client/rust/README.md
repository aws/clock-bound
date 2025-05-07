# Rust example program

This directory contains the source code for an example program in Rust that shows how to obtain error bounded timestamps from the ClockBound daemon and VMClock. This example program makes use of the `clock-bound-client` Rust crate.

## Prerequisites

The ClockBound daemon must be running for the example to work.

The VMClock kernel driver must be installed and available for the example to work.

See the [ClockBound daemon documentation](../clock-bound-d/README.md) for details on how to get the ClockBound daemon running.

See the [VMClock documentation](../clock-bound-d/README.md#VMClock) for how to get the VMClock kernel driver installed.

## Building with Cargo

Run the following command to build the example program.

```sh
cargo build --release
```

## Running the example after a Cargo build

Run the following commands to run the example program.

```sh
cd target/release/
./clock-bound-vmclock-client-example
```

The output should look something like the following:

```sh
$ ./clock-bound-vmclock-client-example
When clockbound_now was called true time was somewhere within 1709840040.474890494 and 1709840040.475674190 seconds since Jan 1 1970. The clock status is "Synchronized".
It took 8.158869083999999 seconds to call clock bound 100000000 times (12256600 tps))
```
