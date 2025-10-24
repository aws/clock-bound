# Test program: link-local-test

This directory contains the source code for a test program written to
validate the implementation of the NTPSource runner. The NTPSource
runner sends NTP packets to a specified NTP host's IP address. 

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
./ntp-server-test
```


The output should look something like the following:

```sh
$ ./ntp-server-test
Lets get a NTP packet!
NTP Server creation complete!
Polling 0
It looks like we got an ntp packet from 1st Source 
Ok(
    Ntp {
        tsc_pre: Time {
            instant: 3299176641588978,
            _marker: PhantomData<clock_bound::daemon::time::tsc::Tsc>,
        },
        tsc_post: Time {
            instant: 3299176649716686,
            _marker: PhantomData<clock_bound::daemon::time::tsc::Tsc>,
        },
        data: NtpData {
            server_recv_time: Time {
                instant: 1761157887289808405000000,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            server_send_time: Time {
                instant: 1761157887289811367000000,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            root_delay: Diff {
                duration: 366210937500,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            root_dispersion: Diff {
                duration: 289916992187,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            stratum: Level(
                ValidStratumLevel(
                    4,
                ),
            ),
        },
    },
)
5 ms
It looks like we got an ntp packet from 2nd Source 
Ok(
    Ntp {
        tsc_pre: Time {
            instant: 3299176641709894,
            _marker: PhantomData<clock_bound::daemon::time::tsc::Tsc>,
        },
        tsc_post: Time {
            instant: 3299176652154514,
            _marker: PhantomData<clock_bound::daemon::time::tsc::Tsc>,
        },
        data: NtpData {
            server_recv_time: Time {
                instant: 1761157887290555565000000,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            server_send_time: Time {
                instant: 1761157887290567787000000,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            root_delay: Diff {
                duration: 335693359375,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            root_dispersion: Diff {
                duration: 335693359375,
                _marker: PhantomData<clock_bound::daemon::time::instant::Utc>,
            },
            stratum: Level(
                ValidStratumLevel(
                    4,
                ),
            ),
        },
    },
)
5 ms
Polling 1
...
Polling 6
```
