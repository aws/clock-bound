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
Lets get PHC samples!
PHC event received.
Ok(
    Phc {
        counter_pre: TscCount(
            1425415815539165,
        ),
        counter_post: TscCount(
            1425415815545741,
        ),
        data: PhcData {
            time: Instant(
                1763144203.339_967_033,
            ),
            clock_error_bound: Duration(
                0.000_016_188,
            ),
        },
    },
)
500 msec

[...]

Expected poll interval duration: 500ms
Actual poll interval duration (i.e. measured amount): 499.986555ms
Margin of error allowed for poll interval duration: 30ms
Poll interval duration difference between actual and expected: 13.445µs
PHC test is successful.
```
