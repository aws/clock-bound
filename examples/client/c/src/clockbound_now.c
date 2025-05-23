// This is an example used to demonstrate how one can use the libclockbound library to retrieve an
// interval of timestamps within which true time exists.

#include <stdio.h>
#include <string.h>
#include <time.h>

#include "clockbound.h"

int CALL_COUNT = 100000000;

/*
 * Helper function to print out errors returned by libclockbound.
 */
void print_clockbound_err(char const* detail, const clockbound_err *err) {
        fprintf(stderr, "%s: ", detail);
        switch (err->kind) {
                case CLOCKBOUND_ERR_NONE:
                        fprintf(stderr, "Success\n");
                        break;
                case CLOCKBOUND_ERR_SYSCALL:
                        if (err->detail) {
                                fprintf(stderr, "%s: %s\n", err->detail, strerror(err->sys_errno));
                        } else {
                                fprintf(stderr, "%s\n", strerror(err->sys_errno));
                        }
                        break;
                case CLOCKBOUND_ERR_SEGMENT_NOT_INITIALIZED:
                        fprintf(stderr, "Segment not initialized\n");
                        break;
                case CLOCKBOUND_ERR_SEGMENT_MALFORMED:
                        fprintf(stderr, "Segment malformed\n");
                        break;
                case CLOCKBOUND_ERR_CAUSALITY_BREACH:
                        fprintf(stderr, "Segment and clock reads out of order\n");
                        break;
                case CLOCKBOUND_ERR_SEGMENT_VERSION_NOT_SUPPORTED:
                        fprintf(stderr, "Segment version not supported\n");
                        break;
                default:
                        fprintf(stderr, "Unexpected error\n");
        }
}

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
 * Helper function to calculate a time interval between two timestamps held in a struct timespec.
 */
double duration(struct timespec start, struct timespec end) {
        int secs = end.tv_sec - start.tv_sec;
        int nsecs = (int) end.tv_nsec - (int) start.tv_nsec;

        if (nsecs < 0) {
                nsecs += 1000000000;
                secs--;
        }

        return  (double) secs + (double) nsecs / 1000000000;
}

int main(int argc, char *argv[]) {
        char const* clockbound_shm_path = CLOCKBOUND_SHM_DEFAULT_PATH;
        char const* vmclock_shm_path = VMCLOCK_SHM_DEFAULT_PATH;
        clockbound_ctx *ctx;
        clockbound_err open_err;
        clockbound_err const* err;
        clockbound_now_result first;
        clockbound_now_result last;
        double dur;
        int i;

        // Open clockbound and retrieve a context on success.
        ctx = clockbound_vmclock_open(clockbound_shm_path, vmclock_shm_path, &open_err);
        if (ctx == NULL) {
                print_clockbound_err("clockbound_open", &open_err);
                return 1;
        }

        // Read the current time reported by the system clock, but as a time interval within which
        // true time exists.
        err = clockbound_now(ctx, &first);
        if (err) {
                print_clockbound_err("clockbound_now", err);
                return 1;
        }

        printf("When clockbound_now was called true time was somewhere within "
               "%ld.%09ld and %ld.%09ld seconds since Jan 1 1970. The clock status is %s.\n",
               first.earliest.tv_sec, first.earliest.tv_nsec,
               first.latest.tv_sec, first.latest.tv_nsec,
               format_clock_status(first.clock_status)
        );

        // Very naive performance benchmark. This is VERY naive, your mileage may vary.
        i = CALL_COUNT;
        while (i > 0) {
                err = clockbound_now(ctx, &last);
                if (err) {
                        print_clockbound_err("clockbound_now", err);
                        return 1;
                }
                i--;
        }

        dur = duration(first.earliest, last.earliest);
        printf("It took %.9lf seconds to call clock bound %d times (%d tps))",
                dur, CALL_COUNT, (int) (CALL_COUNT / dur));


        // Finally, close clockbound.
        err = clockbound_close(ctx);
        if (err) {
                print_clockbound_err("clockbound_close", err);
                return 1;
        }

        return 0;
}
