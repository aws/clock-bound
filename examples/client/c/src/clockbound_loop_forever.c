// This is an example used to demonstrate how one can use the libclockbound library to retrieve an
// interval of timestamps within which true time exists.

#include <stdio.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "clockbound.h"


/*
 * Helper function to convert clock status codes into a human readable version.
 */
char * format_clock_status(clockbound_clock_status status) {
        switch (status) {
                case CLOCKBOUND_STA_UNKNOWN:
                        return "UNKNOWN";
                case CLOCKBOUND_STA_SYNCHRONIZED:
                        return "SYNCHRONIZED";
                case CLOCKBOUND_STA_FREE_RUNNING:
                        return "FREE_RUNNING";
                case CLOCKBOUND_STA_DISRUPTED:
                        return "DISRUPTED";
                default:
                        return "BAD CLOCK STATUS";
        }
}

/*
 * Helper function to convert clockbound error kind codes into a human readable version.
 */
char * format_err_kind(clockbound_err_kind kind) {
        switch (kind) {
                case CLOCKBOUND_ERR_NONE:
                        return "No error";
                case CLOCKBOUND_ERR_SYSCALL:
                        return "Syscall error";
                case CLOCKBOUND_ERR_SEGMENT_NOT_INITIALIZED:
                        return "Segment not initialized error";
                case CLOCKBOUND_ERR_SEGMENT_MALFORMED:
                        return "Segment malformed error";
                case CLOCKBOUND_ERR_CAUSALITY_BREACH:
                        return "Causality breach error";
                case CLOCKBOUND_ERR_SEGMENT_VERSION_NOT_SUPPORTED:
                        return "Segment version not supported error";
                default:
                        return "BAD ERROR KIND";
        }
}

/*
 * Helper function to print out errors returned by libclockbound.
 */
void print_clockbound_err(const clockbound_err *err) {
        if (err == NULL) {
                return;
        }
        switch (err->kind) {
                case CLOCKBOUND_ERR_SYSCALL:
                        fprintf(stderr, "%s(%d): %s: %s\n", format_err_kind(err->kind),
                                err->errno, strerror(err->errno), err->detail);
                        break;
                case CLOCKBOUND_ERR_NONE:
                case CLOCKBOUND_ERR_SEGMENT_MALFORMED:
                case CLOCKBOUND_ERR_SEGMENT_NOT_INITIALIZED:
                case CLOCKBOUND_ERR_SEGMENT_VERSION_NOT_SUPPORTED:
                case CLOCKBOUND_ERR_CAUSALITY_BREACH:
                        fprintf(stderr, "%s(%d): %s\n", format_err_kind(err->kind),
                                err->errno, err->detail);
                        break;
                default:
                        fprintf(stderr, "Unexpected error\n");
        }
}

int main(int argc, char *argv[]) {
        char const* clockbound_shm_path = CLOCKBOUND_SHM_DEFAULT_PATH;
        char const* vmclock_shm_path = VMCLOCK_SHM_DEFAULT_PATH;
        clockbound_ctx *ctx;
        clockbound_err cb_err;
        clockbound_err const* err;
        clockbound_now_result first;
        clockbound_now_result last;
        double dur;
        int i;

        // Open clockbound and retrieve a context on success.
        ctx = clockbound_vmclock_open(clockbound_shm_path, vmclock_shm_path, &cb_err);
        if (ctx == NULL) {
                print_clockbound_err(&cb_err);
                return 1;
        }

        while (1) {
                // Read the current time reported by the system clock, but as a time interval within which
                // true time exists.
                err = clockbound_now(ctx, &first, &cb_err);
                if (err) {
                        print_clockbound_err(err);
                        return 1;
                }

                printf("When clockbound_now was called true time was somewhere within "
                       "%ld.%09ld and %ld.%09ld seconds since Jan 1 1970. The clock status is %s (%d).\n",
                       first.earliest.tv_sec, first.earliest.tv_nsec,
                       first.latest.tv_sec, first.latest.tv_nsec,
                       format_clock_status(first.clock_status),
                       first.clock_status
                );

                sleep(1);
        }

        // Finally, close clockbound.
        err = clockbound_close(ctx, &cb_err);
        if (err) {
                print_clockbound_err(err);
                return 1;
        }

        return 0;
}
