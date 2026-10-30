import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import SegmentLayoutDiagram from '../../components/SegmentLayoutDiagram';
import { siteConfig } from '../../data/site';

const fields: [string, string, string][] = [
  ['Magic Number', 'u64', 'Signature identifying a ClockBound shared memory segment.'],
  ['Segment Size', 'u32', 'Size of the shared memory segment, in bytes.'],
  ['Min Version', 'u8', 'Oldest layout version backward-compatible with this one.'],
  ['Max Version', 'u8', 'Most recent layout version supported.'],
  [
    'Generation',
    'u16',
    'Incremented around updates: odd before an update, even after. On rollover it wraps to 2, not 0.',
  ],
  ['As-Of TSC', 'u64', 'The TSC counter value used to create the as-of timestamp.'],
  ['As-Of Timestamp', 'i64, i64', 'The calculated timestamp (tv_sec, tv_nsec) at the TSC value.'],
  [
    'Void-After Timestamp',
    'i64, i64',
    'Time after which the bound should not be trusted (tv_sec, tv_nsec).',
  ],
  ['Period', 'u64', 'Fractional part of the estimated TSC period.'],
  ['Period Error', 'u64', 'Fractional part of the relative error of the period estimate.'],
  ['Bound', 'i64', 'Absolute upper bound on accuracy vs. reference time at the as-of timestamp (ns).'],
  ['Disruption Marker', 'u64', 'Last disruption marker value read from the VMClock.'],
  ['Max Drift', 'u32', 'Maximum clock drift rate between updates, in parts per billion (ppb).'],
  ['Clock Status', 'i32', '0 Unknown, 1 Synchronized, 2 FreeRunning, 3 Disrupted.'],
  ['Clock Disruption Support', 'u8', '1 if clock-disruption support is enabled, else 0.'],
  ['Period Shift', 'u8', 'Scale applied to Period.'],
  ['Period Error Shift', 'u8', 'Scale applied to Period Error.'],
];

const protocolMatrix: [string, string, string][] = [
  ['1.x', 'shm (v1)', '/var/run/clockbound/shm'],
  ['2.x', 'shm (v2)', '/var/run/clockbound/shm0'],
  ['3.x', 'shm (v3)', '/var/run/clockbound/shm1'],
];

export default function DocsProtocol() {
  return (
    <DocsLayout
      title="Protocol"
      description="The ClockBound shared-memory protocol (version 3). Use it to build a custom client in any language that can mmap a file."
    >
      <AnchorHeading level={2} id="overview">Overview</AnchorHeading>
      <p>
        Protocol version 3 corresponds to ClockBound daemon and client releases{' '}
        <code>3.0.0</code> and greater. Communication between daemon and client
        happens entirely through shared memory, mapped by default to a file at{' '}
        <code>/var/run/clockbound/shm1</code>.
      </p>
      <p>
        Byte ordering is the native endianness of the CPU — for example, x86_64
        and ARM-based Graviton CPUs are little-endian.
      </p>

      <AnchorHeading level={2} id="layout">Segment layout</AnchorHeading>
      <p>The version-3 segment is 104 bytes, laid out as shown below.</p>

      {/* Packet-style visualization of the 104-byte segment, above the
          field-by-field table. Driven by src/data/segmentLayout.ts. */}
      <SegmentLayoutDiagram />

      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="bg-base-700/60 text-slate-300">
            <tr>
              <th className="px-4 py-2 font-semibold">Field</th>
              <th className="px-4 py-2 font-semibold">Type</th>
              <th className="px-4 py-2 font-semibold">Description</th>
            </tr>
          </thead>
          <tbody>
            {fields.map((f, i) => (
              <tr key={f[0]} className={i % 2 ? 'bg-base-800/40' : 'bg-base-800/10'}>
                <td className="px-4 py-2 font-medium text-white">{f[0]}</td>
                <td className="px-4 py-2 font-mono text-cyan-glow">{f[1]}</td>
                <td className="px-4 py-2 text-slate-400">{f[2]}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

      <AnchorHeading level={2} id="generation">Reading consistently</AnchorHeading>
      <p>
        The <strong>Generation</strong> field lets a reader detect a torn read.
        It is set to an odd number before the daemon updates the segment and an
        even number after. A client should read the generation, read the
        payload, then read the generation again: if it changed or is odd, retry.
      </p>

      <AnchorHeading level={2} id="computing">Computing the clock error bound at read time</AnchorHeading>
      <p>
        Reading the TSC (<code>rdtsc</code> on x86, <code>cntvct_el0</code> on
        aarch64) gives <code>now_tsc</code>. With the two scaled fractions from
        the segment:
      </p>
      <pre>
        <code>{`period     = Period / 2^(64 + Period Shift)              seconds per tick
period_err = Period Error / 2^(64 + Period Error Shift)  dimensionless
elapsed    = (now_tsc - As-Of TSC) * period * 1e9        nanoseconds`}</code>
      </pre>
      <p>the current time and its bound, both in nanoseconds, are:</p>
      <pre>
        <code>{`now      = As-Of Timestamp + elapsed
bound    = Bound + elapsed * (Max Drift * 1e-9 + period_err)
earliest = now - bound
latest   = now + bound`}</code>
      </pre>
      <p>
        Both growth terms are rates applied to the elapsed time: the
        oscillator&apos;s worst-case drift (Max Drift, in ppb) and the relative
        error of the period estimate. The result is only meaningful before the{' '}
        <strong>Void-After Timestamp</strong> and while the{' '}
        <strong>Clock Status</strong> permits.
      </p>

      <AnchorHeading level={2} id="older-versions">Older protocol versions</AnchorHeading>
      <p>
        Protocol versions 1 and 2 (and a pre-1.0 Unix datagram socket protocol)
        are documented in the repository&apos;s{' '}
        <a
          href={`${siteConfig.githubUrl}/blob/main/docs/protocol.md`}
          target="_blank"
          rel="noopener noreferrer"
        >
          protocol reference
        </a>
        . Version 3 is the current layout and the one new clients should target.
      </p>

      <AnchorHeading level={2} id="compatibility">Version compatibility</AnchorHeading>
      <p>
        ClockBound&apos;s shared-memory layout has evolved across major
        versions. The daemon and client must agree on the protocol; new clients
        should target version 3.
      </p>
      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="bg-base-700/60 text-slate-300">
            <tr>
              <th className="px-4 py-2 font-semibold">Release</th>
              <th className="px-4 py-2 font-semibold">Protocol</th>
              <th className="px-4 py-2 font-semibold">Default shm path</th>
            </tr>
          </thead>
          <tbody>
            {protocolMatrix.map((row, i) => (
              <tr key={row[0]} className={i % 2 ? 'bg-base-800/40' : 'bg-base-800/10'}>
                <td className="px-4 py-2 font-medium text-white">{row[0]}</td>
                <td className="px-4 py-2 text-slate-400">{row[1]}</td>
                <td className="px-4 py-2 font-mono text-cyan-glow">{row[2]}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="mt-4">
        As of the 3.0 line, support for ClockBound 2.x clients is deprecated and
        will be removed in a future release. Plan migrations to the 3.x clients.
      </p>
    </DocsLayout>
  );
}
