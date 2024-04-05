# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - TODO_FILL_IN_DATE

### Changed

- The communication mechanism used in the ClockBound daemon with clients has
  changed from using Unix datagram socket to using shared memory.

- The communication mechanism used to communicate between the ClockBound daemon
  and Chrony has changed from UDP to Unix datagram socket.

- ClockBound daemon must be run as the chrony user so that it can communicate
  with Chrony.

### Removed

- Removed support for ClockBound clients that are using the *clock-bound-c* library
  which communicates with the ClockBound daemon using Unix datagram socket.

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

No changes, dependency bump only.

## [0.1.0] - 2021-11-02

### Added

- Initial working version
