[![Crates.io](https://img.shields.io/crates/v/clock-bound-ffi.svg)](https://crates.io/crates/clock-bound-ffi)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

# ClockBound Foreign Function Interface (FFI)

This crate implements the FFI for ClockBound. It builds into the libclockbound c library that
an application can use to communicate with the ClockBound daemon.

## Usage

clock-bound-ffi requires ClockBound daemon to be running to work.
See [ClockBound daemon documentation](../clock-bound-d/README.md) for installation instructions.

### Building

Run the following to build the source code of this crate:

```sh
cargo build --release
```

It produces `libclockbound.a`, `libclockbound.so`

- Copy `clock-bound-ffi/include/clockbound.h` to `/usr/include/`
- Copy `target/release/libclockbound.a` to `/usr/lib/`
- Copy `target/release/libclockbound.so` to `/usr/lib/`

### Example

Source code of a runnable c example program can be found at [../examples/c](../examples/c).
See the [README.md](../examples/c/README.md) in that directory for more details on how to
build and run the example.

## Updating README

This README is generated via [cargo-readme](https://crates.io/crates/cargo-readme). Updating can be done by running:

```sh
cargo readme > README.md
```

## Security

See [CONTRIBUTING](../CONTRIBUTING.md#security-issue-notifications) for more information.

## License

Licensed under the [Apache 2.0](LICENSE) license.
