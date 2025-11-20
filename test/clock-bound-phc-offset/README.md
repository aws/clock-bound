# Test program: clock-bound-phc-offset
This directory contains the source code for a test program written to
compare offsets of specified clocks against PHC timestamp reads.

## Prerequisites

AWS EC2 instance is required.

On non-Amazon Linux distributions, the ENA Linux driver will need to be installed and configured with support for the PHC enabled:
- https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configure-ec2-ntp.html#connect-to-the-ptp-hardware-clock
- https://github.com/amzn/amzn-drivers/tree/master/kernel/linux/ena


The PTP hardware clock (PHC) device must have read permissions for the user that is running the phc-test program.

```sh
sudo chmod 644 /dev/ptp0
```


## Building with Cargo

Run the following command to build the test program.

```sh
cargo build --release
```

## Running the program after a Cargo build

Run the following commands to run the test program.

```sh
cd target/release/
sudo ./clock-bound-phc-offset --clock real-time
```

Example output:
```
{"timestamp":"2025-11-19T18:58:07.662842Z","level":"INFO","fields":{"message":"OffsetRttAndCeb { offset_and_rtt: ClockOffsetAndRtt { offset: Duration(0.000_007_372), rtt: Duration(0.000_007_997) }, ceb: Duration(0.000_020_808) }"},"target":"clock_bound_phc_offset"}
```
