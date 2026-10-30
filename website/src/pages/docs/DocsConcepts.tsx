import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import ClockErrorBoundWidget from '../../components/ClockErrorBoundWidget';
import { glossary } from '../../data/content';

export default function DocsConcepts() {
  return (
    <DocsLayout
      title="Concepts"
      description="The core ideas behind ClockBound: the clock error bound, the window of uncertainty, and clock status."
    >
      <AnchorHeading level={2} id="clock-error-bound">The Clock Error Bound</AnchorHeading>
      <p>
        The <strong>Clock Error Bound (CEB)</strong> is a measure of clock
        accuracy. It is defined against reference time (UTC) and represents the
        maximum difference between the local clock on your computer and that
        reference time.
      </p>
      <p>
        Crucially, the clock error bound is <strong>not</strong> the offset
        against the sources ClockBound tracks. Instead it reports the worst
        case: every source of error accumulated along the path from the
        reference clock to your computer. That makes it a general metric for
        comparing timestamps across nodes, independent of their location or
        synchronization protocol.
      </p>

      <AnchorHeading level={2} id="window-of-uncertainty">The window of uncertainty</AnchorHeading>
      <p>
        For every event of interest, a ClockBound client reports a{' '}
        <strong>window of uncertainty</strong> defined by two timestamps,{' '}
        <code>earliest</code> and <code>latest</code>, that bound reference
        time. The window is simply the time reported by the daemon, plus or
        minus the clock error bound.
      </p>
      <p>
        The bound grows between clock updates as the local oscillator drifts,
        and shrinks again when a fresh reference measurement arrives. When two
        nodes&apos; windows do not overlap, their order is certain; when they
        do, an application knows it must not assume an order.
      </p>

      <ClockErrorBoundWidget />

      <AnchorHeading level={2} id="clock-status">Clock status</AnchorHeading>
      <p>
        Alongside the time window, the daemon reports a clock status so that
        clients can know whether the time window can be trusted.
      </p>
      <ul>
        <li>
          <code>0 — Unknown</code>: the status of the clock is unknown.
          Error-bounded timestamps should <strong>not</strong> be trusted.
        </li>
        <li>
          <code>1 — Synchronized</code>: the clock is kept accurate by the
          synchronization daemon. Error-bounded timestamps{' '}
          <strong>can be trusted</strong>.
        </li>
        <li>
          <code>2 — FreeRunning</code>: the clock is free running and not being
          updated by the synchronization daemon. Error-bounded timestamps{' '}
          <strong>can be trusted</strong>.
        </li>
        <li>
          <code>3 — Disrupted</code>: the clock has been disrupted and its
          accuracy cannot be bounded. Error-bounded timestamps should{' '}
          <strong>not</strong> be trusted.
        </li>
      </ul>

      <AnchorHeading level={2} id="disruption">Clock disruption &amp; the VMClock</AnchorHeading>
      <p>
        On virtual machines, maintenance events can disrupt the clock — and the
        underlying clock hardware may even change — violating assumptions made
        by time-synchronization software. ClockBound reads the{' '}
        <strong>VMClock</strong> (a vDSO-style clock exposed to guests at{' '}
        <code>/dev/vmclock0</code>) to detect these disruptions and reflect them
        in the clock status, so applications avoid acting on untrustworthy time.
      </p>

      <AnchorHeading level={2} id="glossary">Glossary</AnchorHeading>
      <dl className="mt-4 space-y-4">
        {glossary.map((g) => (
          <div
            key={g.term}
            className="rounded-xl border border-base-600 bg-base-800/60 p-4"
          >
            <dt className="font-semibold text-white">{g.term}</dt>
            <dd className="mt-1 text-sm text-slate-400">{g.def}</dd>
          </div>
        ))}
      </dl>
    </DocsLayout>
  );
}
