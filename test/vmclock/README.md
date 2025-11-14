# Test program: vmclock-test

This directory contains the source code for a test program written to validate
the implementation of the VMCock runner. The VMClock runner loads the VMClock
shared memory file and polls the file for clock disruption events.


## Prerequisites

This program must be run on an AWS instance supports the vmclock device.

## Building with Cargo

Run the following command to build the test program.

```sh
cargo build --release
```

## Running the program after a Cargo build

Run the following commands to run the test program.

```sh
cd target/release/
./vmclock-test
```


The output should look something like the following:

```sh
$ ./vmclock-test

Attempting to create vmclock runner using /dev/vmclock0 file.
VMClock running initiation and polling test PASSED.

```
