# ClockBound Shared Memory

## Overview
This crate implements the low-level IPC functionality to share ClockErrorBound data and clock status over a shared memory segment. It provides a reader and writer implementation to facilitate operating on the shared memory segment.

## Clock status

Clock status are retrieved directly from `chronyd` tracking data.

- `Unknown`: the status of the clock is unknown.
- `Synchronized`: the clock is kept accurate by the synchronization daemon.
- `FreeRunning`: the clock is free running and not updated by the synchronization daemon.

## Finite State Machine (FSM)
FSM drives a change in the clock status word stored in the ClockBound shared memory segment. Each transition in the FSM is triggered by `chrony`. See following state diagram for clock status in shared memory:

![State Diagram for ClockStatus in SHM](../docs/assets/FSM.png)

## Errors returned by all low-level ClockBound APIs

- `SyscallError(Errno, &'static CStr)`: a system call failed.
  - variant includes the Errno struct with error details
  - an indication on the origin of the system call that error'ed.
- `SegmentNotInitialized`: the shared memory segment is not initialized.
- `SegmentMalformed`: the shared memory segment is initialized but malformed.
- `CausalityBreach`: failed causality check when comparing timestamps.
