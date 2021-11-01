[![Crates.io](https://img.shields.io/crates/v/clock-bound-d.svg)](https://crates.io/crates/clock-bound-d)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

# ClockBoundC

A client library to communicate with ClockBoundD.
## Usage
ClockBoundC requires ClockBoundD to be running to work. See [ClockBoundD documentation](../clock-bound-d/README.md) for installation instructions.

For Rust programs built with Cargo, add "clock-bound-c" as a dependency in your Cargo.toml.

For example:
```
[dependencies]
clock-bound-c = "0.1.0"
```

### Examples

Runnable examples exist at [examples](examples) and can be run with Cargo.

"/run/clockboundd/clockboundd.sock" is the expected default clockboundd.sock location, but the examples can be run with a
different socket location if desired:

```
cargo run --example now /run/clockboundd/clockboundd.sock
cargo run --example before /run/clockboundd/clockboundd.sock
cargo run --example after /run/clockboundd/clockboundd.sock
```

## Updating README

This README is generated via [cargo-readme](https://crates.io/crates/cargo-readme). Updating can be done by running:
```
cargo readme > README.md
```

## Security

See [CONTRIBUTING](../CONTRIBUTING.md#security-issue-notifications) for more information.

## License

Licensed under the [Apache 2.0](LICENSE) license.
