import { Link } from 'react-router-dom';
import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import DocTable from '../../components/DocTable';
import TerminalWindow from '../../components/TerminalWindow';
import { siteConfig } from '../../data/site';

const envelopeExample = `{
  "timestamp": "2026-07-16T14:34:00.000000Z",
  "event": "<event_type>",
  "seq": 42,
  "<event_type>": {
    ...
  },
  "version": "3.0.0"
}`;

const initExample = `{
  "timestamp": "2026-07-16T14:30:00.000000Z",
  "event": "init",
  "seq": 0,
  "init": {
    "message": "Clock Sync Algorithm initialized"
  },
  "version": "3.0.0"
}`;

const disruptionExample = `{
  "timestamp": "2026-07-16T15:00:00.000000Z",
  "event": "disruption",
  "seq": 18234,
  "disruption": {
    "message": "Disruption occurred"
  },
  "version": "3.0.0"
}`;

const ntpFeedExample = `{
  "timestamp": "2026-07-16T14:33:49.654418Z",
  "event": "feed",
  "seq": 1287,
  "feed": {
    "input": {
      "type": "ntp",
      "identifier": "169.254.169.123:123",
      "counter_pre": 482393264419,
      "counter_post": 482393405301,
      "data": {
        "server_recv_time_ns": 1784212429654307727,
        "server_send_time_ns": 1784212429654319674,
        "root_delay_ns": 30517,
        "root_dispersion_ns": 15258,
        "stratum": 1
      }
    },
    "output": null
  },
  "version": "3.0.0"
}`;

const phcFeedExample = `{
  "timestamp": "2026-07-16T14:33:49.654572Z",
  "event": "feed",
  "seq": 1288,
  "feed": {
    "input": {
      "type": "phc",
      "identifier": "/dev/ptp0",
      "counter_pre": 482393526053,
      "counter_post": 482393528552,
      "data": {
        "time_ns": 1784212429654498913,
        "clock_error_bound_ns": 21119
      }
    },
    "output": {
      "counter_sync": 482393527302,
      "time_sync_ns": 1784212429654499150,
      "clock_error_bound_ns": 30007,
      "period_s": 9.524024204451513e-10,
      "period_max_error_s": 7.277185320026017e-16
    }
  },
  "version": "3.0.0"
}`;

const ntpInputExample = `"input": {
  "type": "ntp",
  "identifier": "169.254.169.123:123",
  "counter_pre": 482393264419,
  "counter_post": 482393405301,
  "data": {
    "server_recv_time_ns": 1784212429654307727,
    "server_send_time_ns": 1784212429654319674,
    "root_delay_ns": 30517,
    "root_dispersion_ns": 15258,
    "stratum": 1
  }
}`;

const phcInputExample = `"input": {
  "type": "phc",
  "identifier": "/dev/ptp0",
  "counter_pre": 482393526053,
  "counter_post": 482393528552,
  "data": {
    "time_ns": 1784212429654498913,
    "clock_error_bound_ns": 21119
  }
}`;

const outputExample = `"output": {
  "counter_sync": 482393527302,
  "time_sync_ns": 1784212429654499150,
  "clock_error_bound_ns": 30007,
  "period_s": 9.524024204451513e-10,
  "period_max_error_s": 7.277185320026017e-16
}`;

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
  [<code>seq</code>, 'integer', 'Event sequence number'],
  [<code>version</code>, 'string', 'ClockBound daemon version'],
];

export default function DocsLogFFEvents() {
  return (
    <DocsLayout
      title="Feed Forward Events Log"
      description="Every clock synchronization input and output, for advanced diagnostics and replay."
    >
      <p>
        The Feed Forward Events Log captures clock synchronization corrections
        produced by the synchronization algorithm based on input events from
        the time sources configured with the ClockBound daemon.
      </p>
      <p>
        The Feed Forward Events Log is useful for advanced diagnostics, enabling
        time synchronization experts to analyze the response of the
        synchronization algorithm when faced with noisy input (network
        variability, system load, environmental changes). The ClockBound clock
        synchronization algorithm can deterministically replay this log using
        the{' '}
        <a href={`${siteConfig.githubUrl}/tree/main/clock-bound-ff-tester`} target="_blank" rel="noopener noreferrer">
          <code>clock-bound-ff-tester</code>
        </a>
        . When new scenarios arise in the algorithm, this log allows developers
        to reproduce the issue and validate fixes with certainty.
      </p>
      <p>
        Most users do not need to read or parse this log. It is not a stable
        format, as it exposes internal components within the clock
        synchronization algorithm. Fields can change from version to version.
      </p>
      <ul>
        <li>
          For time synchronization history, see{' '}
          <Link to="/docs/daemon/logging/synchronization">
            <code>synchronization.log</code>
          </Link>
          .
        </li>
        <li>
          For a periodic summary of source status, see{' '}
          <Link to="/docs/daemon/logging/sources">
            <code>sources.log</code>
          </Link>
          .
        </li>
      </ul>

      <AnchorHeading level={2} id="location">Log location</AnchorHeading>
      <TerminalWindow title="path" code={`<log_directory>/ffevents.log`} />
      <ul>
        <li>
          <strong>Format:</strong> Newline-delimited JSON (one JSON object per
          line)
        </li>
        <li>
          <strong>Configuration:</strong> Optional (recommended for expert
          analysis). Enable it with{' '}
          <Link to="/docs/daemon/configuration#logging-ffevents">
            <code>[logging.ffevents]</code>
          </Link>
          ; <code>&lt;log_directory&gt;</code> is{' '}
          <Link to="/docs/daemon/configuration#logging-log-directory">
            <code>logging.log_directory</code>
          </Link>
          .
        </li>
        <li>
          <strong>Volume:</strong> High - one entry per time synchronization
          event per source (multiple entries per second depending on poll rates)
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
            'ISO 8601 UTC timestamp, with microsecond precision, of when the event was emitted',
          ],
          [
            <code>event</code>,
            'string',
            <>
              The type of event, one of <code>init</code>,{' '}
              <code>disruption</code>, <code>feed</code>
            </>,
          ],
          [
            <code>seq</code>,
            'integer',
            <>
              Event sequence number. Starts at <code>0</code> with the{' '}
              <code>init</code> event and increases by one with every following{' '}
              <code>feed</code> or <code>disruption</code> event.
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
          [<code>init</code>, 'Rare (startup-only)', 'The daemon started or restarted'],
          [
            <code>disruption</code>,
            'Rare',
            'A clock disruption was detected and handled by the ClockBound daemon.',
          ],
          [
            <code>feed</code>,
            'High (every input)',
            'A time source event was processed, possibly updating the clock synchronization parameters and accuracy bound.',
          ],
        ]}
      />

      <AnchorHeading level={2} id="init">
        Event: <code>init</code>
      </AnchorHeading>
      <p>
        Marks daemon startup. Delimits boundaries between separate runs when
        processing log files.
      </p>
      <AnchorHeading level={3} id="init-example">Example</AnchorHeading>
      <TerminalWindow title="json" code={initExample} />
      <AnchorHeading level={3} id="init-fields">Fields</AnchorHeading>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          ...commonFields('init'),
          [
            <code>init.message</code>,
            'string',
            <>
              Always <code>&quot;Clock Sync Algorithm initialized&quot;</code>
            </>,
          ],
        ]}
      />
      <p>
        When replaying log files, <code>init</code> lines indicate where a new
        daemon session begins. All prior state should be discarded at this
        boundary.
      </p>

      <AnchorHeading level={2} id="disruption">
        Event: <code>disruption</code>
      </AnchorHeading>
      <p>
        Indicates a clock disruption has been detected and handled. The daemon
        discards buffered events and resets synchronization state when this
        occurs.
      </p>
      <AnchorHeading level={3} id="disruption-example">Example</AnchorHeading>
      <TerminalWindow title="json" code={disruptionExample} />
      <AnchorHeading level={3} id="disruption-fields">Fields</AnchorHeading>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          ...commonFields('disruption'),
          [
            <code>disruption.message</code>,
            'string',
            <>
              Always <code>&quot;Disruption occurred&quot;</code>
            </>,
          ],
        ]}
      />
      <p>
        After a disruption, all <code>feed</code> events will temporarily show
        a <code>null</code> output until sufficient new measurements are
        collected to re-establish clock parameters.
      </p>

      <AnchorHeading level={2} id="feed">
        Event: <code>feed</code>
      </AnchorHeading>
      <p>
        Feed lines are the bulk of the log. Each one records an input event (NTP
        response, PHC reading, etc.) fed into the clock synchronization
        algorithm along with the resulting clock parameters if the time source
        is selected.
      </p>
      <AnchorHeading level={3} id="feed-example-ntp">Example - NTP event (not selected)</AnchorHeading>
      <TerminalWindow title="json" code={ntpFeedExample} />
      <AnchorHeading level={3} id="feed-example-phc">
        Example - PHC event (selected, shows synchronization output)
      </AnchorHeading>
      <TerminalWindow title="json" code={phcFeedExample} />

      <AnchorHeading level={3} id="feed-fields">Feed fields</AnchorHeading>
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          ...commonFields('feed'),
          [
            <code>feed.input</code>,
            'object',
            <>
              The input event (see <a href="#feed-input">Input details</a> below)
            </>,
          ],
          [
            <code>feed.output</code>,
            'object or null',
            <>
              Per-source clock parameters after processing, or null if
              insufficient data or not the selected source (see{' '}
              <a href="#feed-output">Output details</a> below).
            </>,
          ],
        ]}
      />
      <p>
        When <code>output</code> is <code>null</code>, the source is either not
        selected by ClockBound or the algorithm does not yet have enough data to
        produce an estimate.
      </p>
      <p>
        The timestamp values (<code>server_recv_time_ns</code>,{' '}
        <code>server_send_time_ns</code>, <code>time_ns</code> and{' '}
        <code>time_sync_ns</code>) are the number of nanoseconds since the
        beginning of UNIX epoch (<code>1970-01-01T00:00:00Z</code>). All other{' '}
        <code>*_ns</code> values are durations in nanoseconds.
      </p>

      <AnchorHeading level={2} id="feed-input">
        Input details: <code>feed.input</code>
      </AnchorHeading>
      <p>
        Each <code>feed.input</code> variant is of a type identified by{' '}
        <code>feed.input.type</code>. The type can be one of <code>ntp</code>{' '}
        (source providing time information over the Network Time Protocol) or{' '}
        <code>phc</code> (source presented as a PTP Hardware Clock device).
      </p>

      <AnchorHeading level={3} id="feed-input-ntp">Type: NTP</AnchorHeading>
      <p>NTP packet exchange with the time source.</p>
      <TerminalWindow title="json" code={ntpInputExample} />
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          [
            <code>type</code>,
            'string',
            <>
              Always <code>ntp</code>
            </>,
          ],
          [
            <code>identifier</code>,
            'string',
            <>
              IP address and port (<code>ip:port</code>) identifying the time
              source
            </>,
          ],
          [
            <code>counter_pre</code>,
            'integer',
            'Hardware counter reading before sending the NTP request',
          ],
          [
            <code>counter_post</code>,
            'integer',
            'Hardware counter reading after receiving the NTP response',
          ],
          [
            <code>data.server_recv_time_ns</code>,
            'integer',
            'NTP server receive timestamp in nanoseconds',
          ],
          [
            <code>data.server_send_time_ns</code>,
            'integer',
            'NTP server send timestamp in nanoseconds',
          ],
          [
            <code>data.root_delay_ns</code>,
            'integer',
            'Root delay reported by the NTP server in nanoseconds',
          ],
          [
            <code>data.root_dispersion_ns</code>,
            'integer',
            'Root dispersion reported by the NTP server in nanoseconds',
          ],
          [
            <code>data.stratum</code>,
            'integer',
            'NTP stratum level (1 = primary reference clock)',
          ],
        ]}
      />

      <AnchorHeading level={3} id="feed-input-phc">Type: PHC</AnchorHeading>
      <p>
        PTP Hardware Clock reading. The hardware counter is read before and
        after the PHC syscall to bracket the measurement.
      </p>
      <TerminalWindow title="json" code={phcInputExample} />
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          [
            <code>type</code>,
            'string',
            <>
              Always <code>phc</code>
            </>,
          ],
          [<code>identifier</code>, 'string', 'Filepath for the PHC device.'],
          [<code>counter_pre</code>, 'integer', 'Hardware counter reading before the PHC read'],
          [<code>counter_post</code>, 'integer', 'Hardware counter reading after the PHC read'],
          [<code>data.time_ns</code>, 'integer', 'PHC time reading'],
          [
            <code>data.clock_error_bound_ns</code>,
            'integer',
            'Hardware-reported clock error bound, in nanoseconds',
          ],
        ]}
      />

      <AnchorHeading level={2} id="feed-output">
        Output details: <code>feed.output</code>
      </AnchorHeading>
      <p>
        The <code>feed.output</code> object contains information about the clock
        synchronization achieved by the ClockBound daemon. Note that this field
        is only present when:
      </p>
      <ol>
        <li>
          the synchronization algorithm has converged to produce a reliable
          clock synchronization output.
        </li>
        <li>
          the input source of the <code>feed</code> event is currently selected
          as the source of time for the algorithm.
        </li>
      </ol>
      <p>
        When present, the <code>feed.output</code> object contains the clock
        parameters representing ClockBound current best estimate of wall-clock
        time.
      </p>
      <AnchorHeading level={3} id="feed-output-example">Example</AnchorHeading>
      <TerminalWindow title="json" code={outputExample} />
      <DocTable
        headers={['Field', 'Type', 'Description']}
        rows={[
          [
            <code>counter_sync</code>,
            'integer',
            'Hardware counter value at the instant the input was processed and a new clock correction applied',
          ],
          [
            <code>time_sync_ns</code>,
            'integer',
            'Wall-clock time at the instant the input was processed and a new clock correction applied',
          ],
          [
            <code>clock_error_bound_ns</code>,
            'integer',
            'Computed clock error bound, in nanoseconds, at the instant the input was processed and a new clock correction applied',
          ],
          [
            <code>period_s</code>,
            'float',
            'Hardware counter oscillator period in seconds per tick',
          ],
          [
            <code>period_max_error_s</code>,
            'float',
            'Maximum error in the period estimate, in seconds per tick',
          ],
        ]}
      />
    </DocsLayout>
  );
}
