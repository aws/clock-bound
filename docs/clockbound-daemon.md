# ClockBound Daemon

The ClockBound daemon `clockbound` keeps the system clock synchronized by accessing local PTP Hardware Clock (PHC) device or
NTP sources, and offers extra information over a shared memory segment.

## Getting Started

### Installing from the github release RPM

Download pre-built binaries from the GitHub releases page. The releases include RPM packages for x86_64 Linux and
aarch64 Linux architectures.

```sh
# Install RPM package (RHEL/CentOS/Amazon Linux)
sudo rpm -i clockbound-*.rpm

# Start the daemon
sudo systemctl enable clockbound
sudo systemctl start clockbound
```

### Building from `crates.io`
`clockbound` can also be installed via `cargo install`.

```sh
cargo install clock-bound --version "3.0.0-alpha.0"
```

From there you can run `clockbound` as a privileged user by calling
```sh
clockbound
```

If you would like to set up a systemd service, you can use the service and associated script found in the clock-bound repo:

https://github.com/aws/clock-bound/blob/main/clock-bound/assets/

And you can copy the `clockbound` directory into the path expected
by the service file with:

```sh
sudo cp ~/.cargo/bin/clockbound /usr/bin/clockbound
```

## Prerequisites

### VMClock

The VMClock is a vDSO-style clock provided to VM guests.

During maintenance events, VM guests may experience a clock disruption and it is possible that the underlying clock hardware is changed.
This violates assumptions made by time-synchronization software running on VM guests. The VMClock allows us to address this problem by
providing a mechanism for user-space applications such as ClockBound to be aware of clock disruptions, and take appropriate actions to
ensure correctness for applications that depend on clock accuracy.

For more details, see the description provided in file [vmclock-abi.h](https://github.com/torvalds/linux/blob/master/include/uapi/linux/vmclock-abi.h).

The VMClock is included by default in:

- Amazon Linux 2 `kernel-5.10.223-211.872.amzn2` and later.
- Amazon Linux 2023 `kernel-6.1.102-108.177.amzn2023` and later.
- Linux kernel `6.13` and later.

If you are running a Linux kernel that is mentioned above, you will see VMClock at file path `/dev/vmclock0`, assuming that the cloud provider supports it for your virtual machine.

Amazon Web Services (AWS) is rolling out VMClock support on EC2, for AWS Graviton, Intel and AMD architectures.

#### VMClock configuration

VMClock at path `/dev/vmclock0` may not have the read permissions needed by ClockBound. Run the following command to add read permissions.

```sh
sudo chmod a+r /dev/vmclock0
```

## PTP Hardware Clock (PHC) Support on EC2

### Configuring the PHC on Linux and Chrony.

Steps to setup the PHC on Amazon Linux and Chrony are provided here:

- https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configure-ec2-ntp.html

On non-Amazon Linux distributions, the ENA Linux driver will need to be installed and configured with support for the PHC enabled:

- https://github.com/amzn/amzn-drivers/tree/master/kernel/linux/ena

Assuming the ENA driver is enabled and the instance/region combination is supported, you can configure the PHC using the
`configure_phc` script located in `clock-bound/assets`.

```sh
# either call with the '-c' flag 
# which configures the PHC to enable on next boot
./clock-bound/assets/configure_phc -c

# Or call without any flags to immediately update
# NOTE: that this will reload the ena driver of the instance
./clock-bound/assets/configure_phc
```

## Testing clock disruption support

### Manual testing - VMClock

ClockBound reads from the VMClock to know that the clock is disrupted.

If you would like to do testing of ClockBound, simulating various VMClock states, one possibility is to use the vmclock-updater CLI tool.

See the vmclock-updater [README.md](../test/vmclock-updater/README.md) for more details.

## Under the Hood

`clockbound` is a batteries-included clock synchronization daemon that runs with zero configuration, and no required expertise
to set clock synchronization low level parameters. Instead, `clockbound` automatically gathers network devices within the
environment and uses them

The daemon has 3 major components:

- IO
- Clock Sync Algorithm, and
- Clock State

### IO

The io component reads from NTP Sources, the PTP Hardware Clock (PHC), and the VMCLock.

#### NTP Sources

`clockbound` automatically configures to read from the Amazon Time Sync Service and `time.aws.com` via NTP.

For the Amazon time sync service, traffic is sent over `169.254.169.123:123`.
For more information on the Amazon time sync service, information can be found
[here](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configure-ec2-ntp.html)

For the `time.aws.com` endpoints, traffic is sent to 2 endpoints over the internet. 
`clockbound` includes NTP extension field 0xFEC2 (from experimental, reserved range 0xF000–0xFFFF) that carries minimal daemon metadata
for future extensions. The metadata includes the ClockBound version and an ephemeral random session ID regenerated at each daemon start.

#### PHC 

`clockbound` automatically detects `PHC` devices on the instance and will synchronize the clock with them when available.

NOTE: `clockbound` PHC support is limited to the PHC enabled from the ENA driver. For more information on this device, see

- https://github.com/amzn/amzn-drivers/tree/master/kernel/linux/ena

#### VMClock

The VMClock is a vDSO-style clock provided to VM guests. `clockbound` will use `/dev/vmclock0` if it is available.

### Clock Sync Algorithm

The Clock Sync Algorithm uses a feed-forward synchronization algorithm to calculate the current time.

It does this by using hardware oscillator counters to measure the local clock drift. It then compares the local counter values to
reference clock timestamp values to measure the local clock drift.

It carries certain advantages including:

- Decoupling IO from the modifications to the system clock
- Decouples the local System Clock from calculating time
- Enables a stronger story for handling disruption events

### Clock State

This component takes the outputs from the Clock Sync Algorithm, and writes the time and clock error bound to shared memory regions
(for ClockBound clients) and disciplines the system clock.
