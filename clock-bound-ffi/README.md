[![Crates.io](https://img.shields.io/crates/v/clock-bound-ffi.svg)](https://crates.io/crates/clock-bound-ffi)
[![License](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

# ClockBound Foreign Function Interface (FFI)

This crate implements the FFI for ClockBound. It builds into the libclockbound C library that an application can use to communicate with the ClockBound daemon.

## Usage

clock-bound-ffi requires ClockBound daemon to be running to work.

See [ClockBound daemon documentation](../clock-bound-d/README.md) for installation instructions.

### Building

Run the following to build the source code of this crate:

```sh
cargo build --release
```

The build will produce files `libclockbound.a` and `libclockbound.so`.

```sh
# Copy header file `clockbound.h` to directory `/usr/include/`.
sudo cp clock-bound-ffi/include/clockbound.h /usr/include/

# Copy library files `libclockbound.a` and `libclockbound.so` to 
# directory `/usr/lib/`.
sudo cp target/release/libclockbound.a target/release/libclockbound.so /usr/lib/
```

### Example

Source code of a runnable c example program can be found at [../examples/client/c](../examples/client/c).

See the [README.md](../examples/client/c/README.md) in that directory for more details on how to build and run the example.

## Security

See [CONTRIBUTING](../CONTRIBUTING.md#security-issue-notifications) for more information.

## License

Licensed under the [Apache 2.0](LICENSE) license.
