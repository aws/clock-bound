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
