# SHM V3 Error Bound Validation Test

Validates that CLOCK_REALTIME always falls within the clock error bounds advertised by ClockBound via shared memory.

ClockBound is compiled with the `test-side-by-side` feature so it does NOT modify the system clock. The NTP client (chrony in CI) continues to discipline CLOCK_REALTIME independently.

## How it works

The test reads the ClockBound SHM before and after reading CLOCK_REALTIME, producing a pair of bound windows that bracket the realtime measurement. The primary assertion is:

```
before.earliest <= CLOCK_REALTIME <= after.latest
```

## Prerequisites

- An EC2 instance with access to the link-local NTP source (`169.254.169.123`)
- An NTP client (e.g., chrony) running and synchronized
- ClockBound daemon running (built with `daemon,test-side-by-side` features)

## Building

```sh
# Build the test binary
cargo build --bin shmv3-error-bound-validation --release

# Build clockbound with side-by-side mode
cargo build --bin clockbound --features "daemon,test-side-by-side" --release
```

## Running

```sh
# Start clockbound
sudo ./target/release/clockbound &

# Run the test (5 minute measurement, 10s warmup)
./target/release/shmv3-error-bound-validation \
  --sampling-loop-duration 300 \
  --warmup-secs 10
```

## CLI options

| Flag | Default | Description |
|------|---------|-------------|
| `--sampling-loop-duration` | (required) | Duration of the sampling loop in seconds (excludes warmup) |
| `--warmup-secs` | `10` | Warmup period before collecting samples |
| `--sample-interval-micros` | `500` | Microseconds between samples |
| `--shm-path` | `/var/run/clockbound/shm1` | Path to the ClockBound SHM segment |
| `--output-path` | `bound_check_results.json` | Path to write the JSON results artifact |

## Output

The test produces:
- A JSON artifact with all individual bound checks and aggregate results
- A human-readable summary printed to stdout
- Exit code 0 on pass, 1 on failure

## Failure modes

| Outcome | Condition |
|---------|-----------|
| `Passed` | All combined assertions held |
| `NoSamplesCollected` | No valid samples were collected (SHM not responding) |
| `CombinedBoundAssertionFailure` | Realtime escaped the combined bound window |
| `ClockStateInvalid` | >10% of samples had invalid clock state (Unknown/Disrupted) |
