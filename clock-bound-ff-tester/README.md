# ClockBound Feed Forward Algorithm Tester

> "What time is it? Game time!" -- Randy Brown

This crate contains code for testing the feed-forward clock synchronization algorithm through simulation and replay.

## Building

The ff-tester depends on the `clock-bound` crate with the `algorithm` feature (not `daemon`),
which allows it to compile on any platform including macOS:

```bash
cargo build -p clock-bound-ff-tester
cargo test -p clock-bound-ff-tester
```

Note: use `-p clock-bound-ff-tester` rather than `--bin`, as other workspace members may pull in
the `daemon` feature which requires Linux.

## Overview

ClockBound's clock synchronization algorithm tracks the time by using input events (NTP Packet responses,
PHC reads) to output the current estimate. Notably, there are no IO calls done within the
clock sync algorithm. No tsc reads nor system clock syscalls. No mocks either. It is basically a black box with inputs, outputs, and 
state. This enables testing faster than time without requiring the overhead of emulation.

**Disclaimer**: The tester here tests ONLY `clockbound`'s clock synchronization algorithm. It does not test the event gathering IO nor the
control of shared memories and system clocks. We have other test beds and fixtures that cover those cases.

## Replays
### Accessing Replays
The `clockbound` daemon automatically writes a logfile to `primer.log.YYYY-MM-DD-HH` (by default found in `/var/log/clockbound`).
Code in this crate can consume these log files to [deterministically replay](#replaying-primer-logs) time synchronization events
from other hosts. The format of the file is new-line delimited json with data. For more information, you can look at the `subscriber` module
in the `clockbound` crate.

#### Replay Format
Each line of the format marks the kind of event:
- Feed: The main event type that shows the input values and data (NTP response packet, PHC read) as well as the updated clock output.
- Init: Application startup event. Used to delimit log streams between application restarts
- Disruption: A clock disruption event occurred. 

### Replaying Primer Logs
This crate has a command that can be called from `clock-bound-ff-tester repro-replay` that can take an input log and replay the events into the current clock sync algorithm and output the
results into another logfile. If the clock sync algorithm is the same as the one that created the original log input, the output should be identical.
Where this adds value is when exploring changes in the clock sync algorithm. By making changes to the underlying algorithm, one can test the effects
of that change against the same scenario.

### Exploring Replays
This crate's binary has a utility `primer-log-to-parquet` to convert the inputs and outputs into a `.parquet` file. This enables easy/fast data exploration
with pandas/polars frameworks (or anything that can read parquet` files).

## Simulations
This crate has the capability to simulate NTP and PHC events and replay them into the clock synchronization algorithm.
One can use this by first creating a local oscillator model with the `oscillator-generator` command, and then modelling the sources with the
`scenario-generator` command. The output of that can be used to experiment with difficult to recreate scenarios.
