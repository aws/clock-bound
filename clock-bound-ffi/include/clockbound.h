// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

#ifndef _CLOCKBOUND_H
#define _CLOCKBOUND_H

#include <time.h>

#define CLOCKBOUND_SHM_DEFAULT_PATH "/var/run/clockbound/shm"

/*
 * ClockBound context structure (opaque).
 *
 * This structure is NOT thread safe, and should not be shared between threads.
 * Put slighly differently, each thread needs to open its own clockbound context.
 */
typedef struct clockbound_ctx clockbound_ctx;

/*
 * Enumeration of error codes.
 */
typedef enum clockbound_err_kind {
        /* No error. */
        CLOCKBOUND_ERR_NONE,
        /* Error returned by a syscall. */
        CLOCKBOUND_ERR_SYSCALL,
        /* A shared memory segment has not been initialized. */
        CLOCKBOUND_ERR_SEGMENT_NOT_INITIALIZED,
        /* A shared memory segment is initialized but malformed. */
        CLOCKBOUND_ERR_SEGMENT_MALFORMED,
        /* The system clock and shared memory segment reads do match expected order. */
        CLOCKBOUND_ERR_CAUSALITY_BREACH,
} clockbound_err_kind;

/*
 * Error type structure.
 */
typedef struct clockbound_err {
        /* The type of error which occurred. */
        clockbound_err_kind kind;
        /* For CLOCKBOUND_ERR_SYSCALL, the errno which was returned by the system. */
        int sys_errno;
        /* For CLOCKBOUND_ERR_SYSCALL, the name of the syscall which errored. May be NULL. */
        const char* detail;
} clockbound_err;

/*
 * Enumeration of clock status.
 */
typedef enum clockbound_clock_status {
	/* The status of the clock is unknown, time cannot be trusted. */
	CLOCKBOUND_STA_UNKNOWN,
	/* The clock is synchronized to a reference time source. */
	CLOCKBOUND_STA_SYNCHRONIZED,
	/* The clock has lost synchronization but time can still be trusted. */
	CLOCKBOUND_STA_FREE_RUNNING,
} clockbound_clock_status;

/*
 * Clockbound result populating by the `clockbound_now()` operation.
 */
typedef struct clockbound_now_result {
	struct timespec earliest;
	struct timespec latest;
	clockbound_clock_status clock_status;
} clockbound_now_result;

/*
 * Open a new context using the daemon-client segment at `shm_path`.
 *
 * Returns a newly-allocated context on success, and NULL on failure. If err is
 * non-null, fills `*err` with error details.
 */
clockbound_ctx* clockbound_open(char const* shm_path, clockbound_err *err);

/*
 * Close and deallocates the context.
 *
 * Returns NULL on success, or a pointer to error details on failure.
 * */
clockbound_err const* clockbound_close(clockbound_ctx *ctx);

/*
 * Return the Clock Error Bound interval.
 *
 * This function is the equivalent of `clock_gettime(CLOCK_REALTIME)` but in the context of
 * ClockBound. It reads the current time from the system clock (C(t)), and calculate the CEB at this
 * instant. This allows to return a pair of timespec structures that define the interval
 *     [(C(t) - CEB), (C(t) + CEB)]
 * in which true time exists. The call also populate an enum capturing the underlying clock status.
 *
 * The clock status MUST be checked to ensure the bound on clock error is trustworthy.
 */
clockbound_err const* clockbound_now(clockbound_ctx *ctx, clockbound_now_result *res);

#endif
