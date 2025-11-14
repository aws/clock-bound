# Test program: phc-test

This directory contains the source code for a test program written to
validate the implementation of the PHC runner.

Upon startup, the PHC runner attempts to locate a PTP hardware clock (PHC)
on the host.  If a PTP hardware clock is found, then the PHC runner
will periodically read the time and clock error bound from it.

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
./phc-test
```


The output should look something like the following:

```sh
[ec2-user@ip-172-31-31-154 ~]$ ./phc-test
Lets get a PHC sample!
It looks like we got an PHC sample
Ok(
    Phc {
        tsc_pre: TscCount(
            834121827837145,
        ),
        tsc_post: TscCount(
            834121827896745,
        ),
        data: PhcData {
            time: Instant(
                1762897830.385_289_585,
            ),
            clock_error_bound: Duration(
                0.000_016_998,
            ),
        },
    },
)
```
