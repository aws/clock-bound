# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [2.0.2] - 2025-07-30

### Removed

- In dependency 'clock-bound-vmclock', the Cargo.toml no longer specifies
  logging level filter features for the 'tracing' crate.

## [2.0.1] - 2025-05-26

## [2.0.0] - 2025-04-21

### Added

- VMClock is utilized in the algorithm for determining the clock status.

- Support for reading ClockBound shared memory format version 2.

- New error enum value `ClockBoundErrorKind::SegmentVersionNotSupported`.

### Changed

- The default ClockBound shared memory path has changed from
  `/var/run/clockbound/shm` to `/var/run/clockbound/shm0`.

### Removed

- Support for reading ClockBound shared memory format version 1.

## [1.0.0] - 2024-04-05

### Added

- Communication with the ClockBound daemon is now performed via shared memory,
  resulting in a large performance improvement.

### Changed

- Types used in the API have changed with this release.

### Removed

- Communication with the ClockBound daemon via Unix datagram socket has been
  removed with this release.

- Prior to 1.0.0, functions now(), before(), after() and timing() were
  supported.  With this release, before(), after() and timing() have been
  removed.

## [0.1.1] - 2022-03-11

### Added

- Support for the `timing` call.

## [0.1.0] - 2021-11-02

### Added

- Initial working version
