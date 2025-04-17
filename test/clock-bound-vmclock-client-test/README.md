# Test program: clock-bound-vmclock-client-test

This directory contains the source code for a test program
that loops forever, periodically obtaining the error-bounded timestamps.
The test program uses the Rust ClockBound client, which obtains
its clock-related information from the ClockBound daemon and VMClock.

This test program makes use of the `clock-bound-client` Rust crate.

## Prerequisites

The ClockBound daemon must be running for the test program to work.

The VMClock kernel driver must be installed and available for the test program to work.

See the [ClockBound daemon documentation](../../clock-bound-d/README.md) for
details on how to get the ClockBound daemon running and how to setup VMClock.

## Building with Cargo

Run the following command to build the example program.

```
cargo build --release
```

## Running the example after a Cargo build

Run the following commands to run the example program.

```
cd target/release/
./clock-bound-vmclock-client-test
```

The output should look something like the following:

```
$ ./clock-bound-vmclock-client-test
When clockbound_now was called true time was somewhere within 1741625616.796875258 and 1741625616.797993658 seconds since Jan 1 1970. The clock status is "Synchronized".
When clockbound_now was called true time was somewhere within 1741625617.796951539 and 1741625617.798072223 seconds since Jan 1 1970. The clock status is "Synchronized".
When clockbound_now was called true time was somewhere within 1741625618.797036462 and 1741625618.798159432 seconds since Jan 1 1970. The clock status is "Synchronized".
```

To stop the program, type `Ctrl-C` which will send a SIGINT to the program and cause it to exit.
