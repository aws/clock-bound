import { Link } from 'react-router-dom';
import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import TerminalWindow from '../../components/TerminalWindow';

export default function DocsDaemon() {
  return (
    <DocsLayout
      title="Daemon"
      description="The clockbound daemon keeps the system clock synchronized and publishes a bounded timestamp over shared memory."
    >
      <AnchorHeading level={2} id="overview">Overview</AnchorHeading>
      <p>
        The <code>clockbound</code> daemon keeps the system clock synchronized
        by reading from a local PTP Hardware Clock (PHC) device or NTP sources,
        and periodically writes the clock status, time and clock error bound to
        a shared-memory segment. It is batteries-included: it runs with zero
        configuration and automatically gathers the reference time sources
        available in its environment.
      </p>
      <p>The daemon has three major architectural components:</p>
      <ul>
        <li>
          <strong>IO</strong>: reads from NTP sources, the PHC, and the
          VMClock.
        </li>
        <li>
          <strong>Clock Sync Algorithm</strong>: a feed-forward algorithm that
          uses hardware oscillator counters to measure local clock drift and
          compares them against reference timestamps.
        </li>
        <li>
          <strong>Clock State</strong>: writes the time and clock error bound
          into the shared-memory regions and disciplines the system clock.
        </li>
      </ul>
      <p>
        The feed-forward design decouples IO from modifications to the system
        clock, decouples the system clock from calculating time, and enables a
        stronger story for handling disruption events.
      </p>

      <AnchorHeading level={2} id="time-sources">Time sources</AnchorHeading>
      <AnchorHeading level={3} id="ntp">NTP</AnchorHeading>
      <p>
        The daemon automatically configures itself to read from the Amazon Time
        Sync Service (link-local <code>169.254.169.123:123</code>)
        and{' '} <code>time.aws.com</code> via NTP. Additional NTP sources can be supplied
        in a configuration file, either as an individual <code>server</code>{' '}
        (IPv4, IPv6, or hostname) or as a <code>pool</code> hostname that
        resolves to several servers.
      </p>
      <p>
        When polling the public <code>time.aws.com</code> endpoints, the daemon
        includes an NTP extension field (<code>0xFEC2</code>) carrying minimal
        metadata: the ClockBound version and an ephemeral random session ID that
        is regenerated at each daemon start.
      </p>
      <AnchorHeading level={3} id="phc">PHC</AnchorHeading>
      <p>
        The daemon automatically detects PHC devices on the instance and
        synchronizes with them when available. PHC support is limited to the PHC
        exposed by the ENA driver on EC2.
      </p>
      <AnchorHeading level={3} id="vmclock">VMClock</AnchorHeading>
      <p>
        If <code>/dev/vmclock0</code> is available, the daemon uses it to be
        informed of clock disruptions. See{' '}
        <a href="/docs/platforms">Platforms</a> for kernel requirements and
        permissions.
      </p>

      <AnchorHeading level={2} id="configuration">Configuration</AnchorHeading>
      <p>
        Without a configuration file, the daemon runs on built-in defaults and
        logs to stdout only. An optional TOML file can be supplied with{' '}
        <code>-c</code>/<code>--configuration</code>:
      </p>
      <TerminalWindow
        title="bash"
        code={`# Run with a configuration file
clockbound -c /etc/clockbound/config.toml

# Validate a configuration file without starting the daemon
clockbound --check-configuration -c /etc/clockbound/config.toml

# Raise the stdout log level from info to debug
clockbound -v`}
      />
      <p className="mt-4">
        A documented example configuration ships in the repository at{' '}
        <code>clock-bound/assets/example-config.toml</code>.
      </p>

      <AnchorHeading level={2} id="cli">Command-line reference</AnchorHeading>
      <ul>
        <li>
          <code>-c</code>, <code>--configuration &lt;FILE&gt;</code> — load a
          TOML configuration file. A file that cannot be read, parsed, or
          validated is reported and the daemon exits non-zero.
        </li>
        <li>
          <code>--check-configuration</code> — load and validate the file passed
          with <code>-c</code>, report whether it is usable, and exit without
          starting.
        </li>
        <li>
          <code>-v</code>, <code>--verbose</code> — raise the stdout log level
          from info to debug.
        </li>
      </ul>

      <AnchorHeading level={2} id="cbctl">cbctl</AnchorHeading>
      <p>
        <code>cbctl</code> is a command-line utility bundled with the daemon.
        Use <code>cbctl status</code> to inspect the clock status and error
        bound, <code>cbctl waitsync</code> to block until the clock is
        synchronized, and <code>cbctl sources</code> to view the state of the
        daemon&apos;s time sources. See the{' '}
        <Link to="/docs/cbctl">cbctl reference</Link> for details.
      </p>

      <AnchorHeading level={2} id="logging">Logging</AnchorHeading>
      <p>
        The daemon can write configurable log streams to the configured log
        directory:
      </p>
      <ul>
        <li>
          <code>synchronization.log</code> — a human-readable clock
          synchronization history, snapshotted every minute.
        </li>
        <li>
          <code>ffevents.log</code> — raw sync data for deterministic expert
          replay analysis.
        </li>
        <li>
          <code>sources.log</code> — a snapshot of every time source&apos;s
          state, written every minute. <code>cbctl sources</code> reads it.
        </li>
      </ul>
      <p>
        Each stream is off by default and is enabled separately in the
        configuration file (for example, <code>[logging.sources]</code> with{' '}
        <code>enabled = true</code>). Enabling any stream requires{' '}
        <code>log_directory</code> to be set under <code>[logging]</code>.
      </p>
      <p>
        The daemon reopens its log files on <code>SIGHUP</code> to support log
        rotation.
      </p>
    </DocsLayout>
  );
}
