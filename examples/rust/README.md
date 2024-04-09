# Rust example program

This directory contains the source code for an example program in Rust
that shows how to obtain error bounded timestamps from the ClockBound daemon.
This example program makes use of the `clock-bound-client` Rust library.

## Prerequisites

The ClockBound daemon must be running for the example to work.
See the [ClockBound daemon documentation](../../clock-bound-d/README.md) for
details on how to get the ClockBound daemon running.

## Building

Run the following command to build the example program.

```
cargo build --release
```

## Running

Run the following commands to run the example program.

```
cd target/release/
./clockbound-client-rust-example
```

The output should look something like the following:

```
$ ./clockbound-client-rust-example
When clockbound_now was called true time was somewhere within 1709851608.422689931 and 1709851608.425030471 seconds since Jan 1 1970. The clock status is "Synchronized".
It took 9.459138252 seconds to call clock bound 100000000 times (10571787 tps).
```
