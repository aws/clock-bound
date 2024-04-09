[![Crates.io](https://img.shields.io/crates/v/clock-bound-client.svg)](https://crates.io/crates/clock-bound-client)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

# ClockBound client library

A client library to communicate with ClockBound daemon. This client library is written in pure Rust.

## Usage

The ClockBound client library requires ClockBound daemon to be running to work. See [ClockBound daemon documentation](../clock-bound-d/README.md) for installation instructions.

For Rust programs built with Cargo, add "clock-bound-client" as a dependency in your Cargo.toml.

For example:

```
[dependencies]
clock-bound-client = "1.0.0"
```

### Examples

Source code of a runnable example program can be found at [../examples/rust](../examples/rust).
See the [README.md](../examples/rust/README.md) in that directory for more details on how to build and run the example.

## Building

Run the following to build the source code of this crate:

```
cargo build
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
