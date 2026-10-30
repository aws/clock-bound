# ClockBound 2.x client API compatibility test

This directory contains a Rust test program that uses the ClockBound 2.x client
API names from `clock_bound::client` (`ClockBoundClient`, `ClockBoundError`,
`ClockBoundErrorKind`, `ClockBoundNowResult`, `CLOCKBOUND_SHM_DEFAULT_PATH`,
`VMCLOCK_SHM_DEFAULT_PATH`). It is the Rust client example as it was before the
example moved to the crate-root names.

Its main purpose is a compile check: it is a workspace member, so `cargo build`
and `cargo make` fail if a change to the `clock-bound` crate breaks code written
against the 2.x API. Do not migrate it to the new names. For an example of the
current API, see [`examples/client/rust`](../../examples/client/rust/).

## Prerequisites

To run the program (not just build it), the ClockBound daemon must be running
and the VMClock kernel driver must be available.

See the [ClockBound daemon documentation](../../docs/clockbound-daemon.md) for
details on how to get the ClockBound daemon running.

See the [VMClock documentation](../../docs/clockbound-daemon.md#vmclock) for how
to get the VMClock kernel driver installed.

## Building with Cargo

```sh
cargo build --release -p clock-bound-v2-client-test
```

## Running the test program after a Cargo build

```sh
cd target/release/
./clock-bound-v2-client-test
```

The output should look something like the following:

```sh
$ ./clock-bound-v2-client-test
When clockbound_now was called true time was somewhere within 1709840040.474890494 and 1709840040.475674190 seconds since Jan 1 1970. The clock status is Synchronized.
It took 8.158869083999999 seconds to call clock bound 100000000 times (12256600 tps)
```
