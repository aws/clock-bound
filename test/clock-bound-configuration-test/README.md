# Test program: clock-bound-configuration-test

This directory contains the source code for a test program `clock-bound-configuration-test`.

### `clock-bound-configuration-test`
This program starts the `clockbound` daemon against configurations it writes to a
temporary directory, and asserts on what the daemon: that a rejected configuration
reaches stderr rendered rather than escaped and stops startup with exit code 2;
that an accepted one produces the stream files it asked for and no others;
and that `-v` makes debug records visible where the default level does not.

Cases run in order and assert as they go, so the first failure stops the run and names
itself.

## Prerequisites

None.

## Building with Cargo

```
cargo build --release
```

## Running `clock-bound-configuration-test`

The build artifacts should show up at
```
./target/release/clock-bound-configuration-test
./target/release/clockbound
```

You can run the command like below, and the output should look similar:
```
$ ./target/release/clock-bound-configuration-test
running missing_file_is_reported_on_stderr
running invalid_syntax_points_at_the_offending_line
running every_semantic_violation_is_listed
running only_the_enabled_stream_is_written
running verbose_makes_debug_records_visible
running debug_records_are_hidden_without_verbose

6 cases passed
```

Pass `--clockbound <path>` to run against a daemon binary built elsewhere.