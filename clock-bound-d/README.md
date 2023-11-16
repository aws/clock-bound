[![Crates.io](https://img.shields.io/crates/v/clock-bound-d.svg)](https://crates.io/crates/clock-bound-d)
[![License: GPL v2](https://img.shields.io/badge/License-GPL%20v2-blue.svg)](https://www.gnu.org/licenses/old-licenses/gpl-2.0.en.html)

# ClockBoundD

A daemon to provide clients with an error bounded timestamp interval.

## Prerequisites

[chronyd](https://chrony.tuxfamily.org/) must be running in order to run ClockBoundD. If running
on Amazon Linux 2, chronyd is already set as the default NTP daemon for you.

If running on Amazon EC2, see the [EC2 User Guide](https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/set-time.html) for more information on installing Chrony and syncing
to the Amazon Time Sync Service.

## Installation
### Cargo
ClockBoundD can be installed using Cargo. Instructions on how to install cargo can be found at
[doc.rust-lang.org](https://doc.rust-lang.org/cargo/getting-started/installation.html).

If it's your first time installing Cargo on an AL2 EC2 instance you may need to also install gcc:
```
sudo yum install gcc
```

Run cargo install:
```
cargo install clock-bound-d
```

If cargo was installed with the rustup link above the default install location will be at
```
$HOME/.cargo/bin/clockboundd
```

### Systemd configuration

If built from source using cargo, it is recommended to set up systemd to manage ClockBoundD.

Configuration Example:

* Move binary to the location you want to run it from
```
sudo mv $HOME/.cargo/bin/clockboundd /usr/local/bin/clockboundd
```

* Create system user that systemd can use
```
sudo useradd -r clockbound
```

* Create unit file /usr/lib/systemd/system/clockboundd.service with the following contents
```
[Unit]
Description=ClockBoundD

[Service]
Type=simple
Restart=always
RestartSec=10
ExecStart=/usr/local/bin/clockboundd
RuntimeDirectory=clockboundd
WorkingDirectory=/run/clockboundd
User=clockbound

[Install]
WantedBy=multi-user.target
```

* Reload systemd
```
sudo systemctl daemon-reload
```

* Enable ClockBoundD to start at boot
```
sudo systemctl enable clockboundd
```

* Start ClockBoundD now
```
sudo systemctl start clockboundd
```

You can then check the status of the service with:
```
systemctl status clockboundd
```

## Usage

To communicate with ClockBoundD a client is required. A rust client library exists at [ClockBoundC](../clock-bound-c/README.md)
that an application can use to communicate with ClockBoundD.

### PTP Hardware Clock (PHC) Support on EC2

To get accurate clock error bound values when `chronyd` is synchronizing to the PHC (since `chronyd` assumes the PHC itself has 0 error bound which is not necesarily true), a PHC reference ID and PHC network interface (i.e. ENA interface like eth0) need to be supplied for ClockBound to read the clock error bound of the PHC and add it to `chronyd`'s clock error bound. This can be done via CLI args `-r` (ref ID) and `-i` (interface). Ref ID is seen in `chronyc tracking`, i.e.:
```
[ec2-user@ip-172-31-25-217 ~]$ chronyc tracking
Reference ID    : 50484330 (PHC0) <-- This 4 character ASCII code
Stratum         : 1
Ref time (UTC)  : Wed Nov 15 18:24:30 2023
System time     : 0.000000014 seconds fast of NTP time
Last offset     : +0.000000000 seconds
RMS offset      : 0.000000060 seconds
Frequency       : 6.614 ppm fast
Residual freq   : +0.000 ppm
Skew            : 0.019 ppm
Root delay      : 0.000010000 seconds
Root dispersion : 0.000001311 seconds
Update interval : 1.0 seconds
Leap status     : Normal
```
and network interface should be the primary network interface (from `ifconfig`, the interface with index 0) - on Amazon Linux 2 this will generally be `eth0`, and on Amazon Linux 2023 this will generally be `ens5`.

For example:
```
/usr/local/bin/clockboundd -r PHC0 -i eth0
```

To have your systemd unit do this, you'll need to edit the above line to supply the right arguments.

For example:
```
[Unit]
Description=ClockBoundD

[Service]
Type=simple
Restart=always
RestartSec=10
ExecStart=/usr/local/bin/clockboundd -r PHC0 -i eth0
RuntimeDirectory=clockboundd
WorkingDirectory=/run/clockboundd
User=clockbound

[Install]
WantedBy=multi-user.target
```

### Custom Client

If you want to create a custom client see [Custom Client](../README.md#custom-client) for more information.

## Logging

By default, ClockBoundD logs to syslog at /var/log/daemon.log.

syslog logs can be viewed with journalctl:
```
journalctl -u clockboundd
```
## Updating README

This README is generated via [cargo-readme](https://crates.io/crates/cargo-readme). Updating can be done by running:
```
cargo readme > README.md
```

## Security

See [CONTRIBUTING](../CONTRIBUTING.md#security-issue-notifications) for more information.

## License

Licensed under the [GPL v2](LICENSE) license.
