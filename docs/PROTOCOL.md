# ClockBound Shared Memory Protocol Version 2

This protocol version corresponds with ClockBound daemon and client releases 2.0.0 and greater.
The communication between the daemon and client are performed via shared memory.
By default the shared memory segment is mapped to a file at path `/var/run/clockbound/shm0`.

## Shared Memory Segment Layout

The byte ordering of data described below is in the native endian of the CPU architecture you are running on.
For example, x86_64 and ARM-based Graviton CPUs use little endian.

```text
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                          Magic Number                         +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                          Segment Size                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|            Version            |           Generation          |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                                                               +
|                                                               |
+                        As-Of Timestamp                        +
|                                                               |
+                                                               +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                                                               +
|                                                               |
+                      Void-After Timestamp                     +
|                                                               |
+                                                               +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                             Bound                             +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                       Disruption Marker                       +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                           Max Drift                           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                          Clock Status                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Clk Disr Supp |            Padding                            |
+-+-+-+-+-+-+-+-+                                               +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

## Description

**Magic Number**: (u64)

The signature that identifies a ClockBound shared memory segment.

`0x41 0x4D 0x5A 0x4E 0x43 0x42 0x02 0x00`

**Segment Size**: (u32)

The size of shared memory segment in bytes.

**Version**: (u16)

The version number identifying the contents and layout of the data in the shared memory segment.

**Generation**: (u16)

The generation number is increased during updates to the shared memory content by the ClockBound daemon.
It is set to an odd number before an update and it is set to an even number after an update is completed.
Upon rolling over the generation is not set to 0 but it is set to 2.

**As-Of Timestamp**: (i64, i64)

The `CLOCK_MONOTONIC_COARSE` timestamp recorded when the bound on clock error was calculated.

The two signed 64-bit integers correspond to a libc::timespec's `tv_sec` and `tv_nsec`.

**Void-After Timestamp**: (i64, i64)

The `CLOCK_MONOTONIC_COARSE` timestamp beyond which the bound on clock error should not be trusted.

The two signed 64-bit integers correspond to a libc::timespec's `tv_sec` and `tv_nsec`.

**Bound**: (i64)

The absolute upper bound on the accuracy of the `CLOCK_REALTIME` clock with regard to true time at the instant represented by the *As-Of Timestamp*. The units of this value is nanoseconds.

**Disruption Marker**: (u64)

The last disruption marker value that the ClockBound daemon has read from the VMClock.

**Max Drift**: (u32)

The maximum drift rate of the clock between updates of the synchronization daemon, represented in parts per billion (ppb).

**Clock Status**: (i32)

The clock status. Possible values are:

0 - Unknown: The status of the clock is unknown.

1 - Synchronized: The clock is kept accurate by the synchronization daemon.

2 - FreeRunning: The clock is free running and not updated by the synchronization daemon.

3 - Disrupted: The clock has been disrupted and the accuracy of time cannot be bounded.

**Clock Disruption Support**: (u8)

The flag which indicates that clock disruption support is enabled.

0 - Clock disruption support is not enabled.

1 - Clock disruption support is enabled.

# ClockBound Shared Memory Protocol Version 1

This protocol version corresponds with ClockBound daemon and client releases 1.0.0 and greater.
The communication between the daemon and client are performed via shared memory.
By default the shared memory segment is mapped to a file at path `/var/run/clockbound/shm`.

## Shared Memory Segment Layout

The byte ordering of data described below is in the native endian of the CPU architecture you are running on.
For example, x86_64 and ARM-based Graviton CPUs use little endian.

```text
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                          Magic Number                         +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                          Segment Size                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|            Version            |           Generation          |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                                                               +
|                                                               |
+                        As-Of Timestamp                        +
|                                                               |
+                                                               +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                                                               +
|                                                               |
+                      Void-After Timestamp                     +
|                                                               |
+                                                               +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                                                               |
+                             Bound                             +
|                                                               |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                           Max Drift                           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                            Reserved                           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                          Clock Status                         |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|                            Padding                            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

## Description

**Magic Number**: (u64)

The signature that identifies a ClockBound shared memory segment.

`0x41 0x4D 0x5A 0x4E 0x43 0x42 0x02 0x00`

**Segment Size**: (u32)

The size of shared memory segment in bytes.

**Version**: (u16)

The version number identifying the contents and layout of the data in the shared memory segment.

**Generation**: (u16)

The generation number is increased during updates to the shared memory content by the ClockBound daemon.
It is set to an odd number before an update and it is set to an even number after an update is completed.
Upon rolling over the generation is not set to 0 but it is set to 2.

**As-Of Timestamp**: (i64, i64)

The `CLOCK_MONOTONIC_COARSE` timestamp recorded when the bound on clock error was calculated.

The two signed 64-bit integers correspond to a libc::timespec's `tv_sec` and `tv_nsec`.

**Void-After Timestamp**: (i64, i64)

The `CLOCK_MONOTONIC_COARSE` timestamp beyond which the bound on clock error should not be trusted.

The two signed 64-bit integers correspond to a libc::timespec's `tv_sec` and `tv_nsec`.

**Bound**: (i64)

The absolute upper bound on the accuracy of the `CLOCK_REALTIME` clock with regard to true time at the instant represented by the *As-Of Timestamp*. The units of this value is nanoseconds.

**Max Drift**: (u32)

The maximum drift rate of the clock between updates of the synchronization daemon, represented in parts per billion (ppb).

**Reserved**: (u32)

Space reserved for future use.

**Clock Status**: (i32)

The clock status. Possible values are:

0 - Unknown: The status of the clock is unknown.

1 - Synchronized: The clock is kept accurate by the synchronization daemon.

2 - FreeRunning: The clock is free running and not updated by the synchronization daemon.

# ClockBound Unix Datagram Socket Protocol Version 1

This protocol version corresponds with ClockBound daemon and client releases prior to 1.0.0. The communication
between the daemon and client are performed via Unix datagram socket.

## Request
### Request Header

| 0 | 1 | 2 | 3 |
|---|---|---|---|
| V | T |RSV|RSV|

V, u8: The protocol version of the request (1).
T, u8: The request type: Now (1), Before (2), After (3).
RSV, u8: Reserved.
RSV, u8: Reserved.

### Now Request

|0 1 2 3 |
|:------:|
|HEADER  |

HEADER: See header definition above. Now request only has the header. T set to Now (1).

### Before/After Request
| 0  1  2  3 | 4 5 6 7 8 9 10 11 |
|:----------:|:-----------------:|
|HEADER      |EPOCH              |

HEADER: See header defintion above. T set to either Before (2) or After (3).
EPOCH, u64: The time we are testing against represented as the number of nanoseconds from the unix epoch (Jan 1 1970 UTC)

## Response
### Response Header
| 0 | 1 | 2 | 3 |
|---|---|---|---|
| V | T | F |RSV|

V, u8: The protocol version of this response.
T, u8: The response type. Should always match a valid request type; otherwise returns Error (0).
F, u8: Set to 1 if Chrony is not synchronized. Set to 0 otherwise.
RSV, u8: Reserved.

### Now Response
| 0  1  2  3 | 4  5  6  7  8  9  10 11| 12 13 14 15 16 17 18 19|
|:----------:|:----------------------:|:----------------------:|
|HEADER      |EARLIEST                |LATEST                  |

HEADER: See header definition above.
EARLIEST, u64: Clock Time - Clock Error Bound represented as the number of nanoseconds from the unix epoch (Jan 1 1970 UTC).
LATEST, u64: Clock Time + Clock Error Bound represented as the number of nanoseconds from the unix epoch (Jan 1 1970 UTC).

### After Response
| 0  1  2  3 | 4   |
|:----------:|:---:|
|HEADER      |A    |

A, u8: Set to 1 (true) if the requested time happened after the latest error bound of the current system time, otherwise 0 (false).

### Before Response
| 0  1  2  3 | 4   |
|:----------:|:---:|
|HEADER      |B    |

B, u8: Set to 1 (true) if the requested time happened before the earliest error bound of the current system time, otherwise 0 (false).

### Error Response
| 0  1  2  3 |
|:----------:|
|HEADER      |

HEADER: See header definition above. An error response returns the header with T set to Error (0).
