# Test program: ntp-source-test

This directory contains the source code for a test program written to
validate the implementation of the `NTPSource` runner. The `NTPSource`
runner sends NTP packets to a specified NTP host's IP address. 

This test creates two `NTPSource`s using our default public time IPs, and confirms that packets are retrieved from both sources.

## Prerequisites

This program must be run on an instance with internet access

## Building with Cargo

Run the following command to build the test program.

```sh
cargo build --release
```

## Running the program after a Cargo build

Run the following commands to run the test program.

```sh
cd target/release/
./ntp-source-test
```


The output should look something like the following:

```sh
$ ./ntp-source-test
NTP Source creation complete!
Lets get NTP packets!
Packet received from host (1 / 2)
Packet received from host (2 / 2)
TEST COMPLETE
```
