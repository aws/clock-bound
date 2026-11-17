import { Link } from 'react-router-dom';
import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import DocTable from '../../components/DocTable';
import TerminalWindow from '../../components/TerminalWindow';

const envelopeExample = `{
  "timestamp": "2026-07-16T14:34:00.000000Z",
  "event": "sources_snapshot",
  "sources_snapshot": {
    "sources": [ ... ]
  },
  "version": "3.0.0"
}`;

const fullExample = `{
  "timestamp": "2026-07-16T14:34:00.000000Z",
  "event": "sources_snapshot",
  "sources_snapshot": {
    "sources": [
      {
        "type": "phc",
        "identifier": "/dev/ptp0",
        "poll_interval_sec": 0.5,
        "health": {
          "selected_at": "2026-07-16T12:00:00.000000Z",
          "deselected_at": "2026-07-16T13:22:01.000000Z",
          "status": "SYNCHRONIZED"
        },
        "stats": {
          "requests": 18240,
          "responses": 18240,
          "latency_ns": { "min": 2400, "median": 2500, "max": 2600 },
          "source_clock_error_bound_ns": { "min": 20000, "median": 21000, "max": 22000 },
          "window_duration_sec": 600
        }
      },
      {
        "type": "ntp",
        "identifier": "169.254.169.123:123",
        "poll_interval_sec": 2.0,
        "health": {
          "selected_at": "2026-07-16T13:22:01.000000Z",
          "deselected_at": null,
          "status": "SYNCHRONIZED"
        },
        "stats": {
          "requests": 4562,
          "responses": 4558,
          "latency_ns": { "min": 128400, "median": 141200, "max": 155000 },
          "source_clock_error_bound_ns": { "min": 27800, "median": 30100, "max": 31200 },
          "window_duration_sec": 600
        }
      },
      {
        "type": "ntp",
        "identifier": "192.0.2.1:123",
        "poll_interval_sec": 16.0,
        "health": {
          "selected_at": null,
          "deselected_at": null,
          "status": "UNKNOWN"
        },
        "stats": {
          "requests": 64,
          "responses": 0,
          "latency_ns": null,
          "source_clock_error_bound_ns": null,
          "window_duration_sec": 0
        }
      }
    ]
  },
  "version": "3.0.0"
}`;

export default function DocsLogSources() {
  return (
    <DocsLayout
      title="Sources Log"
      description="A periodic snapshot of every time synchronization source and its current state."
    >
      <p>
        The sources log provides a periodic snapshot of all configured time
        synchronization sources and their current state. It is designed for
        compliance auditing, monitoring the health of redundant sources, and
        understanding why one source is selected over another.
      </p>
      <p>
        This log also serves as the backend for the{' '}
        <Link to="/docs/daemon/cbctl#sources">
          <code>cbctl sources</code>
        </Link>{' '}
        CLI command.
      </p>

      <AnchorHeading level={2} id="location">Log location</AnchorHeading>
      <TerminalWindow title="path" code={`<log_directory>/sources.log`} />
      <ul>
        <li>
          <strong>Format:</strong> Newline-delimited JSON (one JSON object per
          line)
        </li>
        <li>
          <strong>Configuration:</strong> Optional (highly recommended). Enable
          it with{' '}
          <Link to="/docs/daemon/configuration#logging-sources">
            <code>[logging.sources]</code>
          </Link>
          ; <code>&lt;log_directory&gt;</code> is{' '}
          <Link to="/docs/daemon/configuration#logging-log-directory">
            <code>logging.log_directory</code>
          </Link>
          .
        </li>
        <li>
          <strong>Requirement:</strong> Without this log enabled, the{' '}
          <code>cbctl sources</code> command will be unusable.
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
            'ISO 8601 UTC timestamp, with microsecond precision, of when the snapshot was taken',
          ],
          [
            <code>event</code>,
            'string',
            <>
              Always <code>sources_snapshot</code>
            </>,
          ],
          [
            <code>sources_snapshot.sources</code>,
            'array',
            'One item per time synchronization source',
          ],
          [<code>version</code>, 'string', 'ClockBound daemon version'],
        ]}
      />
      <DocTable
        headers={['Event', 'Frequency', 'Description']}
        rows={[
          [
            <code>sources_snapshot</code>,
            'Every 60 seconds',
            'An array where each item describes a single time synchronization source',
          ],
        ]}
      />

      <AnchorHeading level={2} id="sources-snapshot">
        Event: <code>sources_snapshot</code>
      </AnchorHeading>
      <p>
        Each item in <code>sources_snapshot.sources</code> has the same keys.
        Information about a source&apos;s identity:
      </p>
      <DocTable
        headers={['Field', 'Type', 'Presence', 'Description']}
        rows={[
          [
            <code>type</code>,
            'string',
            'Always',
            <>
              Source type: <code>ntp</code>, or <code>phc</code>
            </>,
          ],
          [
            <code>identifier</code>,
            'string',
            'Always',
            'Network address of the NTP server (ip:port) or filepath of the PHC device',
          ],
          [
            <code>poll_interval_sec</code>,
            'number',
            'Always',
            'How often this source is polled, in seconds',
          ],
          [<code>health</code>, 'object', 'Always', <a href="#health">Health</a>],
          [<code>stats</code>, 'object', 'Always', <a href="#stats">Stats</a>],
        ]}
      />

      <AnchorHeading level={3} id="health">Health</AnchorHeading>
      <p>The source&apos;s inferred status and selection history.</p>
      <DocTable
        headers={['Field', 'Type', 'Presence', 'Description']}
        rows={[
          [
            <code>health.selected_at</code>,
            'string',
            <>
              Can be <code>null</code> if source has never been selected
            </>,
            'ISO 8601 UTC timestamp of when the time source was selected',
          ],
          [
            <code>health.deselected_at</code>,
            'string',
            <>
              Can be <code>null</code> if source has never been selected or is
              currently selected
            </>,
            'ISO 8601 UTC timestamp of when the time source was deselected',
          ],
          [
            <code>health.status</code>,
            'string',
            'Always',
            <>
              The inferred clock synchronization status of the upstream source
              clock, one of <code>SYNCHRONIZED</code>, <code>FREE_RUNNING</code>,
              or <code>UNKNOWN</code>
            </>,
          ],
        ]}
      />

      <AnchorHeading level={3} id="status">Status</AnchorHeading>
      <p>
        The inferred clock status of the source is determined from its last
        response, measured against the clock of the currently selected source:
      </p>
      <ul>
        <li>
          <code>SYNCHRONIZED</code> - if the source provided a response in the
          last 60 seconds and the last response was healthy.
        </li>
        <li>
          <code>FREE_RUNNING</code> - if the source provided a response in the
          last 60 seconds but the last response was unsynchronized.
        </li>
        <li>
          <code>UNKNOWN</code> - if the source has not provided any response in
          the last 60 seconds, if its last response was unusable for another
          reason, or if no source is currently selected.
        </li>
      </ul>

      <AnchorHeading level={3} id="stats">Stats</AnchorHeading>
      <p>
        Aggregate statistics about the source&apos;s quality, network distance,
        and responsiveness.
      </p>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          [
            <code>stats.requests</code>,
            'integer',
            'Number of requests made to the source since it started',
          ],
          [
            <code>stats.responses</code>,
            'integer',
            'Number of responses from the source since it started',
          ],
          [
            <code>stats.latency_ns</code>,
            <>
              <a href="#stat-summary">stats</a> or null
            </>,
            <>
              Statistical summary of sample read latencies over the window.{' '}
              <code>null</code> if there are no samples in the window, or no
              source is currently selected.
            </>,
          ],
          [
            <code>stats.source_clock_error_bound_ns</code>,
            <>
              <a href="#stat-summary">stats</a> or null
            </>,
            <>
              Statistical summary of sample source clock error bound values over
              the window. <code>null</code> if there are no samples in the
              window.
            </>,
          ],
          [
            <code>stats.window_duration_sec</code>,
            'integer',
            "Duration in seconds for the current snapshot's look-back window, up to 600 seconds (10 minutes).",
          ],
        ]}
      />

      <AnchorHeading level={3} id="stat-summary">Stats summary</AnchorHeading>
      <p>
        Summary statistics for a particular value over the window duration, in
        nanoseconds.
      </p>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          [<code>min</code>, 'integer', 'Minimum value.'],
          [<code>median</code>, 'integer', 'Median value.'],
          [<code>max</code>, 'integer', 'Maximum value.'],
        ]}
      />

      <AnchorHeading level={2} id="full-example">Full example</AnchorHeading>
      <TerminalWindow title="json" code={fullExample} />
    </DocsLayout>
  );
}
