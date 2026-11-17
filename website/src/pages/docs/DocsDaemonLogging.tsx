import { Link } from 'react-router-dom';
import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import DocTable from '../../components/DocTable';

export default function DocsDaemonLogging() {
  return (
    <DocsLayout
      title="Logging Overview"
      description="The ClockBound daemon's log streams, and which one to use for common tasks."
    >
      <p>
        ClockBound outputs logs across several distinct streams, each serving a
        different purpose and audience. This page provides an overview of all
        log streams and guidance on which to use for common tasks.
      </p>

      <AnchorHeading level={2} id="quick-reference">Quick reference</AnchorHeading>
      <DocTable
        headers={['Log Stream', 'Location', 'Format', 'Default', 'Purpose']}
        rows={[
          [
            <a href="#stdout-stderr">Stdout/Stderr</a>,
            'stdout/stderr',
            'Plain text',
            'Always on',
            'Startup, shutdown, clock status changes, errors',
          ],
          [
            <Link to="/docs/daemon/logging/synchronization">
              <code>synchronization.log</code>
            </Link>,
            <code>&lt;log_directory&gt;</code>,
            'NDJSON',
            'Disabled',
            'Clock synchronization information',
          ],
          [
            <Link to="/docs/daemon/logging/sources">
              <code>sources.log</code>
            </Link>,
            <code>&lt;log_directory&gt;</code>,
            'NDJSON',
            'Disabled',
            'Periodic snapshot summary of all sources',
          ],
          [
            <Link to="/docs/daemon/logging/ffevents">
              <code>ffevents.log</code>
            </Link>,
            <code>&lt;log_directory&gt;</code>,
            'NDJSON',
            'Disabled',
            'Feed Forward Events: synchronization algorithm data for advanced diagnostics and replay',
          ],
        ]}
      />
      <p>
        Use the{' '}
        <Link to="/docs/daemon/configuration#logging">
          ClockBound configuration file
        </Link>{' '}
        to enable specific log streams and set their destination,{' '}
        <code>&lt;log_directory&gt;</code>, with{' '}
        <code>logging.log_directory</code>.
      </p>

      <AnchorHeading level={2} id="which-log">Which log to enable?</AnchorHeading>
      <p>
        <strong>
          &quot;I want to confirm the ClockBound daemon correctly detected my
          environment&quot;
        </strong>{' '}
        → Check stdout/stderr to confirm the daemon is running and the
        configuration it applied. Startup failures, permission mismatch and
        unexpected runtime errors can also be found here.
      </p>
      <p>
        <strong>&quot;I want to confirm I am tracking time&quot;</strong> →
        Start with <code>synchronization.log</code>. It captures periodic
        snapshots of ClockBound&apos;s internal clock and summaries of system
        clock adjustments.
      </p>
      <p>
        <strong>&quot;I want to know where my time comes from&quot;</strong> →
        Check <code>sources.log</code>. It provides a periodic summary of all
        configured time sources and their status. The{' '}
        <Link to="/docs/daemon/cbctl#sources">
          <code>cbctl sources</code>
        </Link>{' '}
        CLI command leverages <code>sources.log</code> for easy inspection.
      </p>
      <p>
        <strong>
          &quot;I am curious about synchronization details for a post-mortem
          diagnosis&quot;
        </strong>{' '}
        → Enable the <code>ffevents.log</code> files. These contain the
        timestamps offered by the time sources, and their processing by
        ClockBound for expert-level deterministic replay and analysis.
      </p>

      <AnchorHeading level={2} id="stdout-stderr">Stdout/Stderr</AnchorHeading>
      <ul>
        <li>
          <strong>Output Location:</strong> <code>stdout</code> /{' '}
          <code>stderr</code>.
        </li>
        <li>
          <strong>Format:</strong> Plain-text.
        </li>
        <li>
          <strong>Configuration:</strong> Always enabled. Verbosity is set with{' '}
          <Link to="/docs/daemon/configuration#verbosity">
            <code>-v</code>/<code>--verbose</code>
          </Link>
          .
        </li>
      </ul>

      <AnchorHeading level={3} id="stdout-stderr-whats-logged">What&apos;s logged</AnchorHeading>
      <p>
        <strong>Stdout</strong> captures:
      </p>
      <ul>
        <li>Daemon startup and shutdown events</li>
        <li>Environment auto-detection details</li>
        <li>Per-source configuration and startup information</li>
        <li>Clock status changes</li>
        <li>Clock disruption events</li>
        <li>Clock selection changes</li>
        <li>
          Signal handlers (like <code>SIGHUP</code>)
        </li>
        <li>
          Warnings and errors
        </li>
      </ul>
      <p>
        <strong>Stderr</strong> captures:
      </p>
      <ul>
        <li>Configuration errors that stop the daemon at startup</li>
        <li>Unexpected errors that terminate the daemon</li>
      </ul>

      <AnchorHeading level={3} id="stdout-stderr-when-to-use">When to use</AnchorHeading>
      <p>
        In steady state, these logs are generally sparse. Clock status changes
        and disruptions should be rare - they represent exceptional events. This
        is the place to look when something is fundamentally wrong (daemon
        won&apos;t start, repeated errors) rather than for ongoing
        synchronization details.
      </p>

      <AnchorHeading level={2} id="synchronization-log">Synchronization Log</AnchorHeading>
      <ul>
        <li>
          <strong>Output Location:</strong>{' '}
          <code>&lt;log_directory&gt;/synchronization.log</code>.
        </li>
        <li>
          <strong>Format:</strong> Newline-delimited JSON.
        </li>
        <li>
          <strong>Configuration:</strong> Optional (highly recommended).
        </li>
      </ul>
      <p>
        The synchronization log is the primary interface for user-driven
        investigation and troubleshooting.
      </p>
      <p>
        The synchronization log presents meaningful snapshot summaries of clock
        quality, the selected source and system clock adjustments, as well as
        changes to clock status.
      </p>
      <p>
        For full format details, see{' '}
        <Link to="/docs/daemon/logging/synchronization">
          Synchronization Log
        </Link>
        .
      </p>
      <AnchorHeading level={3} id="synchronization-log-whats-logged">What&apos;s logged</AnchorHeading>
      <ul>
        <li>
          <strong>Daemon Start</strong> (<code>daemon_started</code>) - marks
          each start of the daemon.
        </li>
        <li>
          <strong>Clock Accuracy</strong> (<code>synchronization</code>) -
          periodic report on clock status and quality, and on the selected
          source: its identity, when it was selected, and its clock error bound
          at that time.
        </li>
        <li>
          <strong>Clock Status Changes</strong> (
          <code>clock_status_change</code>) - record of change of clock status.
        </li>
        <li>
          <strong>System Clock Adjustments</strong> (
          <code>system_clock_summary</code>, <code>system_clock_step</code>) -
          periodic report on corrections applied to the operating system clock,
          and a record of any step applied at startup.
        </li>
      </ul>
      <AnchorHeading level={3} id="synchronization-log-when-to-use">When to use</AnchorHeading>
      <p>Use this log to report on:</p>
      <ul>
        <li>The clock accuracy that can be achieved (the Clock Error Bound)</li>
        <li>
          The time source that provides the most reliable time and why it is
          selected
        </li>
        <li>What clock information is exposed to libclockbound consumers</li>
        <li>What system clock adjustments were made</li>
      </ul>

      <AnchorHeading level={2} id="sources-log">Sources Log</AnchorHeading>
      <ul>
        <li>
          <strong>File:</strong> <code>&lt;log_directory&gt;/sources.log</code>.
        </li>
        <li>
          <strong>Format:</strong> Newline-delimited JSON.
        </li>
        <li>
          <strong>Configuration:</strong> Optional (highly recommended).
        </li>
      </ul>
      <p>
        The sources log provides a periodic snapshot summary of all configured
        time synchronization sources. It also serves as the backend for the
        ClockBound client CLI command (
        <Link to="/docs/daemon/cbctl#sources">
          <code>cbctl sources</code>
        </Link>
        ).
      </p>
      <p>
        For full format details, see{' '}
        <Link to="/docs/daemon/logging/sources">Sources Log</Link>.
      </p>
      <AnchorHeading level={3} id="sources-log-whats-logged">What&apos;s logged</AnchorHeading>
      <p>Each snapshot includes a per-source summary with:</p>
      <ul>
        <li>
          <strong>Selection status</strong> - when this source was last
          selected and deselected as the clock.
        </li>
        <li>
          <strong>Poll rate</strong> - duration between successive timestamps
          retrieved from the source.
        </li>
        <li>
          <strong>Source status</strong> - the inferred clock synchronization
          status of the source, from its latest response.
        </li>
        <li>
          <strong>Statistical summary</strong> - Clock Error Bound and Request
          Latency statistical summaries (minimum, median, maximum), and request
          and response counts.
        </li>
      </ul>
      <AnchorHeading level={3} id="sources-log-when-to-use">When to use</AnchorHeading>
      <p>Use this log for:</p>
      <ul>
        <li>
          Compliance auditing and verify which time sources are in use and their
          configuration.
        </li>
        <li>
          Source health monitoring, availability and quality metrics for all
          time sources.
        </li>
        <li>
          Debugging source issues, identifying sources that have stopped
          responding or are producing degraded measurements
        </li>
      </ul>

      <AnchorHeading level={2} id="ffevents-log">Feed Forward Events Log</AnchorHeading>
      <ul>
        <li>
          <strong>File:</strong> <code>&lt;log_directory&gt;/ffevents.log</code>.
        </li>
        <li>
          <strong>Format:</strong> Newline-delimited JSON.
        </li>
        <li>
          <strong>Configuration:</strong> Optional (recommended for in-depth
          analysis).
        </li>
      </ul>
      <p>
        The Feed Forward Events Log captures every clock synchronization event
        processed by the feed-forward clock synchronization algorithm. It is
        designed for advanced diagnostics and replay by time synchronization
        experts.
      </p>
      <p>
        For full format details, see{' '}
        <Link to="/docs/daemon/logging/ffevents">Feed Forward Events Log</Link>.
      </p>
      <AnchorHeading level={3} id="ffevents-log-whats-logged">What&apos;s logged</AnchorHeading>
      <ul>
        <li>
          <strong>Time source input</strong> - Collections of timestamps
          retrieved from the time source (NTP timestamps, PHC read, etc.).
        </li>
        <li>
          <strong>Clock Synchronization output</strong> - The clock built from
          processing information from the selected time source
        </li>
      </ul>
      <AnchorHeading level={3} id="ffevents-log-when-to-use">When to use</AnchorHeading>
      <p>
        The Feed Forward Events Log is of interest to expert users who want to
        dive deeper into the details of the clock synchronization strategy
        implemented by the ClockBound daemon.
      </p>
      <p>
        This is particularly useful to replay a stream of events when running
        post-mortem investigations, or in-depth diagnosis analysis.
      </p>

      <AnchorHeading level={2} id="rotation">Log rotation</AnchorHeading>
      <p>
        All file-based log streams (<code>synchronization</code>,{' '}
        <code>sources</code>, <code>ffevents</code>) require management via{' '}
        <code>logrotate</code> or equivalent. Logs should be configured with
        appropriate rotation policies based on your retention needs.
      </p>
      <p>
        Note: ClockBound registers a <code>SIGHUP</code> handler that closes and
        re-opens its log file descriptors in order to allow for lossless log
        rotation via a <code>postrotate</code> script.
      </p>

      <AnchorHeading level={2} id="configuration">Configuration</AnchorHeading>
      <p>
        Log streams can be individually enabled or disabled via{' '}
        <Link to="/docs/daemon/configuration#logging">
          ClockBound configuration
        </Link>
        . Disabling any log stream reduces the information available for
        troubleshooting. Log streams other than stdout/stderr are disabled by
        default.
      </p>
    </DocsLayout>
  );
}
