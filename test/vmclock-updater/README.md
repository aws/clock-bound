# Test program: vmclock-updater

This directory contains the source code for a test program written in Rust that allows a user to simulate various VMClock states to validate the expected behavior in ClockBound.

The vmclock-updater creates or updates a file at path `/dev/vmclock0` which adheres to the same data layout of VMClock.

## Prerequisites

The real VMClock device on Linux at path `/dev/vmclock0` must not exist.

If the VMClock device (`ptp_vmclock`) was built and installed as a loadable module and not built-in, then you can remove it by running `sudo rmmod ptp_vmclock`.

If the VMClock device (`ptp_vmclock`) is built-in, then this vmclock-updater test tool cannot be used unless the Linux kernel is rebuilt without `ptp_vmclock` built-in.

## Building with Cargo

Run the following command to build the test program.

```sh
cargo build --release
```

## Running the program after a Cargo build

Run the following commands to run the test program.

```sh
cd target/release/
./vmclock-updater --help
```

The output should look something like the following:

```sh
$ sudo ./vmclock-updater --disruption-marker=4
Successfully wrote the following VMClockShmBody to the VMClock shared memory segment: VMClockShmBody { counter_id: 0, counter_value: 0, counter_period_frac_sec: 0, counter_period_error_rate_frac_sec: 0, utc_time_sec: 0, utc_time_frac_sec: 0, utc_time_maxerror_picosec: 0, tai_offset_sec: 0, counter_value_leapsecond: 0, tai_time_sec_leapsecond: 0, leapsecond_direction: 0, clock_status: Unknown, disruption_marker: 4 }
```
