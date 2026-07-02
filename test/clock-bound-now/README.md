# Test program: clock-bound-now

This directory contains the source code for a test program `clock-bound-now`.

### `clock-bound-now`
This program reads the SHM segment and the VMClock marker and outputs the earliest and latest
timestamps (in nanoseconds), the status of the clock, and the "error bound" itself. It outputs this
to stdout as JSON to be used in scripts.

## Prerequisites

The ClockBound daemon should be running, or a valid SHM segment/VMClock path must be supplied otherwise.

## Building with Cargo

Run the following command to build the example programs.

```
cargo build --release
```

## Running `clock-bound-now`

The build artifact should show up at
```
./target/release/clock-bound-now
```

You can run the command like below, and the output should look similar:
```
$ ./target/release/clock-bound-now --client-version (v2|v3)
{"timestamp":"2025-11-17T22:57:30.108786Z","level":"INFO","fields":{"bound":642300,"earliest":1763420250108133684,"latest":1763420250109418284,"clock_status":"Synchronized"},"target":"clock_bound_now"}
```
