import { Link } from 'react-router-dom';
import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';

export default function DocsDaemon() {
  return (
    <DocsLayout
      title="Daemon Overview"
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
        in a <Link to="/docs/daemon/configuration#sources">configuration file</Link>, either as an individual <code>server</code>{' '}
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
        <Link to="/docs/platforms">Platforms</Link> for kernel requirements and
        permissions.
      </p>

      <AnchorHeading level={2} id="configuration">Configuration</AnchorHeading>
      <p>
        Without a configuration file, the daemon runs on built-in defaults and
        auto-detected time sources, logging to stdout/stderr only. An optional
        TOML file, passed with <code>-c</code>/<code>--configuration</code>,
        turns on log file streams and adds NTP time sources. See{' '}
        <Link to="/docs/daemon/configuration">Configuration</Link> for every
        setting.
      </p>

      <AnchorHeading level={2} id="cli">Command-line reference</AnchorHeading>
      <ul>
        <li>
          <code>-c</code>, <code>--configuration &lt;FILE&gt;</code> — load a
          TOML configuration file. A file that cannot be read, parsed, or
          validated is reported on stderr and the daemon exits with code{' '}
          <code>2</code>.
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
        <Link to="/docs/daemon/cbctl">cbctl reference</Link> for details.
      </p>

      <AnchorHeading level={2} id="logging">Logging</AnchorHeading>
      <p>
        The daemon always logs to stdout/stderr. It can also write three log
        file streams to the configured log directory:
      </p>
      <ul>
        <li>
          <Link to="/docs/daemon/logging/synchronization">
            <code>synchronization.log</code>
          </Link>{' '}
          — clock synchronization information: periodic snapshots of clock
          quality and summaries of system clock adjustments.
        </li>
        <li>
          <Link to="/docs/daemon/logging/sources">
            <code>sources.log</code>
          </Link>{' '}
          — a periodic snapshot summary of all sources.{' '}
          <code>cbctl sources</code> reads it.
        </li>
        <li>
          <Link to="/docs/daemon/logging/ffevents">
            <code>ffevents.log</code>
          </Link>{' '}
          — the Feed Forward Events Log: synchronization algorithm data for
          advanced diagnostics and replay.
        </li>
      </ul>
      <p>
        Each stream is disabled by default and is enabled separately in the{' '}
        <Link to="/docs/daemon/configuration#logging">configuration file</Link>.
        See the <Link to="/docs/daemon/logging">Logging overview</Link> for
        which log to enable and how to rotate them.
      </p>
    </DocsLayout>
  );
}
