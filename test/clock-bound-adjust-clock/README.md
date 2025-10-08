# Test program: clock-bound-adjust-clock

This directory contains the source code for a test program `clock-bound-adjust-clock`.

`clock-bound-adjust-clock` is a lightweight wrapper over our internal `ClockState`
component's `adjust_clock` function. It allows for the user to supply a
given phase correction and skew, just like the internal parameters of
the program, in order to pass these values to the kernel via `adjtimex`
system call.

It can be useful for testing of the clock adjustment component itself,
validating that the phase correction and skew are applied correctly if
the CLOCK_REALTIME is compared with a separate reference source (e.g. if
I supply a `skew` of 0 but a `phase_correction` of 1000 nanoseconds, I
should see that CLOCK_MONOTONIC_RAW and CLOCK_REALTIME eventually reach
an offset of +1000 nanoseconds after the call)

## Prerequisites

The program must be run as a user with sufficient permissions to adjust the clock (generally `root`).

Currently only Linux is supported.

## Building with Cargo

Run the following command to build the example program.

```
cargo build --release
```

## Running the example after a Cargo build

Run the following commands to run the example program.

```
cd target/release/
./clock-bound-adjust-clock
```

The output should look something like the following:

```
$ ./clock-bound-adjust-clock --skew-ppb 1000 --phase-correction-seconds 0.5
Applied +65536 frequency setting (+1000 ppb) and +0.5 second phase correction to kernel to slew/correct.
```
