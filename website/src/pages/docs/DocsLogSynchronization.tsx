import { Link } from 'react-router-dom';
import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import DocTable from '../../components/DocTable';
import TerminalWindow from '../../components/TerminalWindow';

const envelopeExample = `{
  "timestamp": "2026-07-16T14:34:00.000000Z",
  "event": "<event_type>",
  "<event_type>": {
    ...
  },
  "version": "3.0.0"
}`;

const daemonStartedExample = `{
  "timestamp": "2026-07-16T14:30:00.120000Z",
  "event": "daemon_started",
  "daemon_started": {
    "message": "ClockBound daemon started"
  },
  "version": "3.0.0"
}`;

const synchronizationExample = `{
  "timestamp": "2026-07-16T14:34:00.000000Z",
  "event": "synchronization",
  "synchronization": {
    "source": {
      "type": "ntp",
      "identifier": "169.254.169.123:123",
      "selected_at": "2026-07-16T13:00:00.000000Z",
      "clock_error_bound_ns": 102000
    },
    "ffclock": {
      "time_sync": "2026-07-16T14:33:59.500000Z",
      "clock_error_bound_ns": 108500,
      "status": "SYNCHRONIZED"
    }
  },
  "version": "3.0.0"
}`;

const synchronizationNullExample = `{
  "timestamp": "2026-07-16T14:30:08.000000Z",
  "event": "synchronization",
  "synchronization": {
    "source": null,
    "ffclock": null
  },
  "version": "3.0.0"
}`;

const summaryExample = `{
  "timestamp": "2026-07-16T14:40:00.000000Z",
  "event": "system_clock_summary",
  "system_clock_summary": {
    "window_width_s": 600,
    "correction_count": 4,
    "correction_ns": {
      "min": -4800,
      "max": -3200,
      "mean": -4100
    }
  },
  "version": "3.0.0"
}`;

const stepExample = `{
  "timestamp": "2026-07-16T14:30:01.200000Z",
  "event": "system_clock_step",
  "system_clock_step": {
    "step_ns": -842000000
  },
  "version": "3.0.0"
}`;

const statusChangeExample = `{
  "timestamp": "2026-07-16T15:22:03.400000Z",
  "event": "clock_status_change",
  "clock_status_change": {
    "from": "SYNCHRONIZED",
    "to": "DISRUPTED"
  },
  "version": "3.0.0"
}`;

const transitions = `UNKNOWN      → SYNCHRONIZED   First clock estimate produced
SYNCHRONIZED → FREE_RUNNING   Latest clock estimate is 60 seconds old or more
FREE_RUNNING → SYNCHRONIZED   Fresh clock estimate produced
any          → DISRUPTED      VMClock disruption event received
DISRUPTED    → SYNCHRONIZED   Recovery after disruption: new clock estimate produced
any          → UNKNOWN        Clock estimate produced while an expected VMClock has failed`;

// Rows shared by every event's field table.
const commonFields = (event: string) => [
  [<code>timestamp</code>, 'string', 'ISO 8601 UTC timestamp of when this entry was emitted'],
  [
    <code>event</code>,
    'string',
    <>
      Always <code>&quot;{event}&quot;</code>
    </>,
  ],
  [<code>version</code>, 'string', 'ClockBound daemon version'],
];

export default function DocsLogSynchronization() {
  return (
    <DocsLayout
      title="Synchronization Log"
      description="The history of the local system's time quality."
    >
      <p>
        The synchronization log is the history of the local system&apos;s time
        quality. It presents snapshot summaries of clock quality, the selected
        source and system clock adjustments, as well as changes to clock
        status.
      </p>

      <AnchorHeading level={2} id="location">Log location</AnchorHeading>
      <TerminalWindow title="path" code={`<log_directory>/synchronization.log`} />
      <ul>
        <li>
          <strong>Format:</strong> Newline-delimited JSON (one JSON object per
          line)
        </li>
        <li>
          <strong>Configuration:</strong> Optional (highly recommended). Enable
          it with{' '}
          <Link to="/docs/daemon/configuration#logging-synchronization">
            <code>[logging.synchronization]</code>
          </Link>
          ; <code>&lt;log_directory&gt;</code> is{' '}
          <Link to="/docs/daemon/configuration#logging-log-directory">
            <code>logging.log_directory</code>
          </Link>
          .
        </li>
      </ul>

      <AnchorHeading level={2} id="event-types">Event types</AnchorHeading>
      <p>
        Each line in the log file is a JSON object. The following top level
        keys are included in every JSON object (examples on this page are
        pretty-printed; in the file each object is on a single line):
      </p>
      <TerminalWindow title="json" code={envelopeExample} />
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          [
            <code>timestamp</code>,
            'string',
            'ISO 8601 UTC timestamp, with microsecond precision, of when the entry was emitted',
          ],
          [
            <code>event</code>,
            'string',
            <>
              One of <code>daemon_started</code>, <code>synchronization</code>,{' '}
              <code>system_clock_summary</code>, <code>system_clock_step</code>,{' '}
              <code>clock_status_change</code>
            </>,
          ],
          [
            <code>&lt;event_type&gt;</code>,
            'object',
            <>
              The event body, keyed by the value of <code>event</code>
            </>,
          ],
          [<code>version</code>, 'string', 'ClockBound daemon version'],
        ]}
      />
      <DocTable
        headers={['Event', 'Frequency', 'Description']}
        rows={[
          [
            <code>daemon_started</code>,
            'Rare (startup-only)',
            'Event indicating that the ClockBound daemon started.',
          ],
          [
            <code>synchronization</code>,
            'Every 60 seconds',
            'ClockBound synchronization summary. Information about the selected time source and the clock synchronization status and accuracy achieved.',
          ],
          [
            <code>system_clock_summary</code>,
            'Every 10 minutes',
            'Aggregate system clock discipline stats. Useful to confirm the system clock is tracking time.',
          ],
          [
            <code>system_clock_step</code>,
            'Rare (startup-only)',
            'Event indicating that the system clock was stepped at daemon startup, because it was more than 100 milliseconds away from the estimated time.',
          ],
          [
            <code>clock_status_change</code>,
            'Rare',
            <>
              Event indicating that the clock status has changed, for example
              from <code>FREE_RUNNING</code> to <code>SYNCHRONIZED</code>.
            </>,
          ],
        ]}
      />
      <p>
        Periodic entries serve as a heartbeat. Event-driven entries fire only on
        exceptional state changes.
      </p>
      <p>
        Event-driven entries (<code>system_clock_step</code>,{' '}
        <code>clock_status_change</code>) may also be emitted to stdout (though
        they will contain less detail in this case).
      </p>

      <AnchorHeading level={2} id="daemon-started">
        Event: <code>daemon_started</code>
      </AnchorHeading>
      <p>Emitted once when the daemon starts.</p>
      <AnchorHeading level={3} id="daemon-started-example">Example</AnchorHeading>
      <TerminalWindow title="json" code={daemonStartedExample} />
      <AnchorHeading level={3} id="daemon-started-fields">Fields</AnchorHeading>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          ...commonFields('daemon_started'),
          [
            <code>daemon_started.message</code>,
            'string',
            <>
              Always <code>&quot;ClockBound daemon started&quot;</code>
            </>,
          ],
        ]}
      />

      <AnchorHeading level={2} id="synchronization">
        Event: <code>synchronization</code>
      </AnchorHeading>
      <p>
        Reports the current selected source and the clock synchronization
        derived from it.
      </p>
      <AnchorHeading level={3} id="synchronization-example">Example</AnchorHeading>
      <TerminalWindow title="json" code={synchronizationExample} />
      <p>
        Before the daemon has selected a source, both <code>source</code> and{' '}
        <code>ffclock</code> are <code>null</code>:
      </p>
      <TerminalWindow title="json" code={synchronizationNullExample} />
      <AnchorHeading level={3} id="synchronization-fields">Fields</AnchorHeading>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          ...commonFields('synchronization'),
          [
            <code>synchronization.source</code>,
            'object or null',
            <>
              The selected time source. <code>null</code> if no source has been
              selected.
            </>,
          ],
          [
            <code>synchronization.source.type</code>,
            'string',
            <>
              Source type, one of <code>ntp</code> or <code>phc</code>
            </>,
          ],
          [
            <code>synchronization.source.identifier</code>,
            'string',
            'IP address and port for an NTP source. Device path for a PHC source.',
          ],
          [
            <code>synchronization.source.selected_at</code>,
            'string',
            'ISO 8601 UTC timestamp of when the time source was selected',
          ],
          [
            <code>synchronization.source.clock_error_bound_ns</code>,
            'integer',
            <>
              The clock error bound of the time source when it was selected at{' '}
              <code>selected_at</code>
            </>,
          ],
          [
            <code>synchronization.ffclock</code>,
            'object or null',
            <>
              The ClockBound daemon clock. <code>null</code> if no source has
              been selected.
            </>,
          ],
          [
            <code>synchronization.ffclock.time_sync</code>,
            'string',
            'ISO 8601 UTC timestamp of the instant at which the ffclock was last synchronized',
          ],
          [
            <code>synchronization.ffclock.clock_error_bound_ns</code>,
            'integer',
            <>
              The Clock Error Bound of the ClockBound daemon when the ffclock was
              synchronized at <code>time_sync</code>
            </>,
          ],
          [
            <code>synchronization.ffclock.status</code>,
            'string',
            <>
              The clock synchronization status of the ClockBound daemon
              algorithm, one of <code>SYNCHRONIZED</code>,{' '}
              <code>FREE_RUNNING</code>, <code>DISRUPTED</code> or{' '}
              <code>UNKNOWN</code>.
            </>,
          ],
        ]}
      />

      <AnchorHeading level={2} id="system-clock-summary">
        Event: <code>system_clock_summary</code>
      </AnchorHeading>
      <p>
        Summarizes system clock discipline activity since the previous
        snapshot. Reports how many corrections were applied and their
        magnitude.
      </p>
      <AnchorHeading level={3} id="system-clock-summary-example">Example</AnchorHeading>
      <TerminalWindow title="json" code={summaryExample} />
      <AnchorHeading level={3} id="system-clock-summary-fields">Fields</AnchorHeading>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          ...commonFields('system_clock_summary'),
          [
            <code>system_clock_summary.window_width_s</code>,
            'integer',
            'Past window size over which statistics are computed (in seconds)',
          ],
          [
            <code>system_clock_summary.correction_count</code>,
            'integer',
            'Number of corrections applied to the system clock over the window',
          ],
          [
            <code>system_clock_summary.correction_ns</code>,
            'object or null',
            <>
              Statistics on corrections, in nanoseconds, applied to the system
              clock over the window: <code>min</code>, <code>max</code> and{' '}
              <code>mean</code> (signed integers). <code>null</code> if no
              correction was applied
            </>,
          ],
        ]}
      />

      <AnchorHeading level={2} id="system-clock-step">
        Event: <code>system_clock_step</code>
      </AnchorHeading>
      <p>
        Emitted when the system clock is stepped (jumped immediately rather than
        slewed gradually). This occurs only at daemon startup, and only when the
        system clock is more than 100 milliseconds away from the estimated time.
        A smaller offset is slewed instead, and no <code>system_clock_step</code>{' '}
        entry is written.
      </p>
      <AnchorHeading level={3} id="system-clock-step-example">Example</AnchorHeading>
      <TerminalWindow title="json" code={stepExample} />
      <AnchorHeading level={3} id="system-clock-step-fields">Fields</AnchorHeading>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          ...commonFields('system_clock_step'),
          [
            <code>system_clock_step.step_ns</code>,
            'integer',
            'The adjustment applied to the system clock, in nanoseconds. Negative means the clock was stepped backward.',
          ],
        ]}
      />
      <p>
        A step of -842ms (as in the example) means the system clock was 842ms
        ahead of the estimated time and was jumped backward at startup.
      </p>

      <AnchorHeading level={2} id="clock-status-change">
        Event: <code>clock_status_change</code>
      </AnchorHeading>
      <p>
        Emitted when the clock&apos;s synchronization status transitions between
        states.
      </p>
      <AnchorHeading level={3} id="clock-status-change-example">Example</AnchorHeading>
      <TerminalWindow title="json" code={statusChangeExample} />
      <AnchorHeading level={3} id="clock-status-change-fields">Fields</AnchorHeading>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          ...commonFields('clock_status_change'),
          [<code>clock_status_change.from</code>, 'string', 'Previous status'],
          [<code>clock_status_change.to</code>, 'string', 'New status'],
        ]}
      />

      <AnchorHeading level={3} id="clock-status-values">Clock status values</AnchorHeading>
      <DocTable
        headers={['Status', 'Meaning']}
        rows={[
          [
            <code>UNKNOWN</code>,
            'Initial state - no clock estimate has been produced yet. Also reported when a VMClock device is expected but has failed.',
          ],
          [
            <code>SYNCHRONIZED</code>,
            'The clock estimate is fresh (updated within the last 60 seconds)',
          ],
          [
            <code>FREE_RUNNING</code>,
            'The clock estimate is stale (no update in ≥ 60 seconds). CEB is growing',
          ],
          [
            <code>DISRUPTED</code>,
            'A VMClock disruption occurred. The clock estimate has been cleared',
          ],
        ]}
      />

      <AnchorHeading level={3} id="status-transitions">Status transitions</AnchorHeading>
      <p>
        The daemon re-evaluates the clock status each time its clock algorithm
        produces a new clock estimate, and when it handles a VMClock disruption:
      </p>
      <TerminalWindow title="transitions" code={transitions} />
      <p>
        A transition to <code>FREE_RUNNING</code> means ClockBound is still
        providing time estimates, but the clock error bound is growing because
        no fresh measurements are available. Applications using the ClockBound
        API will see an increasing (but still valid) CEB until synchronization
        is restored.
      </p>
    </DocsLayout>
  );
}
