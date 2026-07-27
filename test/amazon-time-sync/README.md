# Test program: amazon-time-sync-test

This directory contains the source code for a test program written to
validate the implementation of the Amazon Time Sync NTP runner. The Amazon
Time Sync NTP runner sends NTP packets to the AWS Amazon Time Sync address.

## Prerequisites

This program must be run on an AWS instance or a computer where `169.254.169.123:123` is mapped to an NTP server to complete successfully.

## Building with Cargo

Run the following command to build the test program.

```sh
cargo build --release
```

## Running the program after a Cargo build

Run the following commands to run the test program.

```sh
cd target/release/
./amazon-time-sync-test
```


The output should look something like the following:

```sh
$ ./amazon-time-sync-test
Lets get a NTP packet!
It looks like we got an ntp packet 
Some(
    Event {
        tsc_pre: Time {
            instant: 1755135610916260,
            _marker: PhantomData<clock_bound::daemon::time::tsc::Tsc>,
        },
        tsc_post: Time {
            instant: 1755135611903996,
            _marker: PhantomData<clock_bound::daemon::time::tsc::Tsc>,
        },
        ntp_data: NtpData {
            server_recv_time: Time {
                instant: 1759936069727275701000000,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            server_send_time: Time {
                instant: 1759936069727290229000000,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            root_delay: Diff {
                duration: 30517578125,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            root_dispersion: Diff {
                duration: 15258789062,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            stratum: 1,
        },
    },
)
```
