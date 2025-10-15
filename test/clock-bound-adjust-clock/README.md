# Test program: clock-bound-adjust-clock

This directory contains the source code for a test program `clock-bound-adjust-clock`.

### `adjust-clock`
`adjust-clock` is a lightweight wrapper over our internal `ClockState`
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

### `step-clock`
`step-clock` is a lightweight wrapper over our internal `ClockState`
component's `step_clock` function. It allows for the user to supply a
given phase correction, which is applied to `CLOCK_REALTIME` via a step.
**DO NOT USE THIS IF IN AN ENVIRONMENT WHICH HAS OTHER SOFTWARE WHICH DO NOT EXPECT CLOCK_REALTIME TO BE STEPPED, UNLESS YOU KNOW WHAT YOU ARE DOING!**

It can be useful for testing of the clock stepping functionality itself -
e.g. if I supply a `--phase-correction-seconds` of `10.0`, I should expect the clock to be
stepped 10 seconds forward in time.

## Prerequisites

The program must be run as a user with sufficient permissions to adjust the clock (generally `root`).

Currently only Linux is supported.

## Building with Cargo

Run the following command to build the example programs.

```
cargo build --release
```

## Running `adjust-clock`

The build artifact should show up at
```
./target/release/adjust-clock
```

You can run the command like below, and the output should look similar:
```
$ ./target/release/adjust-clock --skew-ppb 1000 --phase-correction-seconds 0.5
Applied +65536 frequency setting (+1000 ppb) and +0.5 second phase correction to kernel to slew/correct.
```

## Running `step-clock`

The build artifact should show up at
```
./target/release/step-clock
```

You can run the command like below, and the output should look similar:
```
$ ./target/release/step-clock --phase-correction-seconds 5.0
Initial time is 2025-10-06T20:38:17.704638277Z
Applied +5 second phase correction to kernel to step CLOCK_REALTIME.
Final time is 2025-10-06T20:38:22.704708241Z
```