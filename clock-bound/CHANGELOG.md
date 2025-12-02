# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [3.0.0-alpha.1] - 2025-12-02

### Changed

- Update client logic to align 'free-running' definition with daemon (60 seconds of stale shared memory segment instead of 5).
- Improved ShmError messages.
- Fix to local stratum interpretation during initialization.

## [3.0.0-alpha.0] - 2025-11-24

### Added

- The `clockbound` daemon auto-detects and synchronizes from NTP time servers and a PHC device if available.
- The `clockbound` daemon supports the VMClock device for EC2 instances running on Linux.
- The `clockbound` daemon maintains the operating system clock synchronized as well as populating its shared memory
  segment with clock estimates.
- The `clockbound` daemon now writes to two distinct shared memory paths: `/var/run/clockbound/shm0` and `/var/run/clockbound/shm1`.
- The ClockBound 3.x clients (Rust and FFI) solely rely on the content of the shared memory segment to return the clock
  status, as well as the current time and associated clock error bound as `earliest/latest` timestamps.

### Changed

- The FFI interface for ClockBound 3.0 clients has been simplified, and error management improved.
- The shared memory segment written to at `/var/run/clockbound/shm1` follows a new layout. See
  [protocol](https://github.com/aws/clock-bound/blob/main/docs/protocol.md) details.

### Deprecated

- The support for ClockBound 2.x clients is deprecated and will be removed in a future release.

### Removed

- The `clockbound` daemon does not require `chronyd`.
- The `clockbound` daemon only accepts the following CLI parameters: `--log-dir`

## [2.0.3] - 2025-08-13

### Changed

- Updates the polling rate of clockbound to be once every 100 milliseconds.

## [2.0.2] - 2025-07-30

### Removed

- In dependency 'clock-bound-vmclock', the Cargo.toml no longer specifies logging level filter features for the
  'tracing' crate.

## [2.0.1] - 2025-05-26

### Changed

- Fix bug in clock status transitions after a clock disruption.
- Log more details when ChronyClient query_tracking fails.
- Documentation:
  - Update clock status documentation.
  - Update finite state machine image to match the underlying source code.

## [2.0.0] - 2025-04-21

### Added

- VMClock is utilized for being informed of clock disruptions. By default, ClockBound requires VMClock.
- CLI option `--disable-clock-disruption-support`. Using this option disables clock disruption support and causes
  ClockBound to skip the VMClock requirement.
- ClockBound shared memory format version 2. This new shared memory format is not backwards compatible with the shared
  memory format used in prior ClockBound releases. See [PROTOCOL.md](../docs/PROTOCOL.md) for more details.

### Changed

- The default ClockBound shared memory path has changed from `/var/run/clockbound/shm` to `/var/run/clockbound/shm0`.

### Removed

- Support for writing ClockBound shared memory format version 1.
- Support for reading ClockBound shared memory format version 1.

## [1.0.0] - 2024-04-05

### Changed

- The communication mechanism used in the ClockBound daemon with clients has changed from using Unix datagram socket to
  using shared memory.
- The communication mechanism used to communicate between the ClockBound daemon and Chrony has changed from UDP to Unix
  datagram socket.
- ClockBound daemon must be run as the chrony user so that it can communicate with Chrony.
- Types used in the API have changed with this release.

### Removed

- Removed support for ClockBound clients that are using the _clock-bound-c_ library which communicates with the
  ClockBound daemon using Unix datagram socket.
- Prior to 1.0.0, client functions now(), before(), after() and timing() were supported. With this release, before(),
  after() and timing() have been removed.

## [0.1.4] - 2023-11-16

### Added

- ClockBound now supports [reading error bound from a PHC device](https://github.com/amzn/amzn-drivers/tree/master/kernel/linux/ena) as exposed from ENA driver
- Bump tokio dependency from 1.18.4 to 1.18.5

## [0.1.3] - 2023-01-11

### Added

- Bump tokio dependency from 1.17.0 to 1.18.4

## [0.1.2] - 2022-03-11

### Added

- Daemon now correctly handles queries originating from abstract sockets.

## [0.1.1] - 2021-12-28

### Added

- Client support for the `timing` call.

## [0.1.0] - 2021-11-02

### Added

- Initial working version
