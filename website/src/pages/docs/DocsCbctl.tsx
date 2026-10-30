import { Link } from 'react-router-dom';
import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import TerminalWindow from '../../components/TerminalWindow';

// cbctl reference. The `status` and `waitsync` content follows the repository's
// docs/cbctl.md (draft PR #333). `sources` is intentionally limited to a short
// description and invocation until its output format is finalized.
//
// TODO(human): `cbctl status` and `cbctl sources` are not on `main` yet (draft
// PRs #330-#333 and the sources.log series). Do not publish this page before
// those merge.

const statusExample = `Clock status                  SYNCHRONIZED
Current time                  2026-09-29T17:42:03.094821000Z
  Clock error bound (+/-)     67.60 us
  Clock error bound growth    15040 ppb
  Counter value               3855752014882

Last clock sync               2026-09-29T17:42:00.694821000Z  (2s ago)
  Clock error bound (+/-)     31.50 us
  Counter value               3847592014882
  Counter period              0.29411765 ns
  Counter period error (+/-)  40 ppb
  Counter frequency           3.4000000 GHz
  Valid until                 2026-09-29T17:58:40.694821000Z  (16m 38s left)

Disruption support            On`;

export default function DocsCbctl() {
  return (
    <DocsLayout
      title="cbctl"
      description="The ClockBound control utility: inspect the clock status, wait for synchronization, and view time sources."
    >
      <AnchorHeading level={2} id="overview">Overview</AnchorHeading>
      <p>
        <code>cbctl</code> is the ClockBound control command-line utility. It
        is built with the daemon and installed as <code>/usr/bin/cbctl</code>.
      </p>

      <AnchorHeading level={2} id="status">cbctl status</AnchorHeading>
      <p>
        <code>cbctl status</code> shows ClockBound&apos;s clock and
        synchronization information. This includes the current clock status,
        current time and clock error bound, as well as details about the last
        clock synchronization event.
      </p>
      <TerminalWindow title="bash" code={`cbctl status`} />

      <AnchorHeading level={3} id="status-example">Example output</AnchorHeading>
      <TerminalWindow title="output" code={statusExample} />
      <p>
        True time is within <strong>Current time</strong> ±{' '}
        <strong>Clock error bound</strong>. The bound grows at{' '}
        <strong>Clock error bound growth</strong>, in parts per billion (ppb),
        until the next clock sync: 1 ppb is 1 ns of bound per second.{' '}
        <strong>Counter value</strong> is the hardware counter reading the time
        was computed from: the TSC on x86_64, the architectural counter on
        aarch64.
      </p>
      <p>
        <strong>Last clock sync</strong> is the sync the current time comes
        from, with its own clock error bound and counter value.{' '}
        <strong>Counter period</strong> is the measured duration of one counter
        tick, and its error applies to <strong>Counter frequency</strong> too.{' '}
        <strong>Valid until</strong> is when the sync stops being valid.
      </p>

      <AnchorHeading level={3} id="status-clock-status">Clock status</AnchorHeading>
      <ul>
        <li>
          <code>SYNCHRONIZED</code>: the last clock sync was less than 60 seconds
          ago.
        </li>
        <li>
          <code>FREE_RUNNING</code>: the last clock sync is more than 60 seconds
          old but still valid.
        </li>
        <li>
          <code>DISRUPTED</code>: the daemon detected a clock disruption and has
          not synced since.
        </li>
        <li>
          <code>UNKNOWN</code>: the daemon published <code>UNKNOWN</code>, or the
          last clock sync is past Valid until.
        </li>
      </ul>
      <p>
        Under <code>DISRUPTED</code> and <code>UNKNOWN</code> the current time
        cannot be trusted, and therefore you will see <code>-</code> printed
        for <strong>Current time</strong>, its <strong>Clock error bound</strong>, and{' '}
        <strong>Clock error bound growth</strong>.
      </p>
      <p>
        Under <code>DISRUPTED</code>, the <strong>Last clock sync</strong> and{' '}
        <strong>Valid until</strong> also drop their time since and time left values.
      </p>
      <p>
        See <Link to="/docs/concepts#clock-status">Concepts</Link> for what each
        clock status means for applications.
      </p>

      <AnchorHeading level={3} id="status-disruption-support">Disruption support</AnchorHeading>
      <ul>
        <li>
          <code>On</code>: The daemon enabled clock disruption detection through
          the VMClock device.
        </li>
        <li>
          <code>Off</code>: The daemon did not enable clock disruption
          detection.
        </li>
      </ul>
      <p>
        An <code>Off</code> value is expected on EC2 bare metal instances, which
        have no VMClock device, and outside of EC2, where the daemon does not
        enable it. On a virtualized EC2 instance, <code>Off</code> means the
        daemon did not detect that it&apos;s running on EC2.
      </p>

      <AnchorHeading level={3} id="status-exit-codes">Exit codes</AnchorHeading>
      <ul>
        <li>
          <code>0</code>: the daemon&apos;s shared memory segment was read,
          whatever the clock status.
        </li>
        <li>
          <code>1</code>: there is nothing to show. The message says what was
          found: no segment, usually because no daemon has run since boot; no
          clock parameters yet, usually because the daemon has not synced; or
          another error, with its cause.
        </li>
      </ul>

      <AnchorHeading level={2} id="waitsync">cbctl waitsync</AnchorHeading>
      <TerminalWindow
        title="bash"
        code={`cbctl waitsync [--system-clock] [--timeout-seconds SECONDS] [--verbose]`}
      />
      <p>
        Waits for the ClockBound daemon to synchronize the clock. By default it
        waits indefinitely. This is useful in startup scripts and health checks.
      </p>
      <p>Options:</p>
      <ul>
        <li>
          <code>--system-clock</code>: also wait for the system clock
          (<code>CLOCK_REALTIME</code>) to be within the tolerance window. The
          tolerance window is the ClockBound time window, widened equally on
          both sides if needed so that it is at least 2 milliseconds wide in
          total.
        </li>
        <li>
          <code>-t</code>, <code>--timeout-seconds SECONDS</code>: the maximum
          time to wait, in seconds. The default, 0, waits indefinitely.
        </li>
        <li>
          <code>-v</code>, <code>--verbose</code>: print the status of each
          polling attempt while waiting. The command polls once per second.
        </li>
      </ul>
      <p>Exit codes:</p>
      <ul>
        <li>
          <code>0</code>: the clock is ready.
        </li>
        <li>
          <code>1</code>: the clock did not become ready within the timeout.
        </li>
        <li>
          <code>2</code>: waiting can never succeed, for example on a
          virtualized EC2 instance where the VMClock device is expected but
          missing.
        </li>
      </ul>

      <AnchorHeading level={2} id="sources">cbctl sources</AnchorHeading>
      <p>
        <code>cbctl sources</code> displays the state of all time
        synchronization sources that are configured within the ClockBound
        daemon.
      </p>
      <TerminalWindow title="bash" code={`cbctl sources`} />
      <p>
        It reads the daemon&apos;s <code>sources.log</code>, so the sources log
        stream must be enabled in the daemon configuration. See{' '}
        <Link to="/docs/daemon#logging">Daemon logging</Link>.
      </p>
    </DocsLayout>
  );
}
