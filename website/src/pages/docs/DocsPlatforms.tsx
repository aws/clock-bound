import DocsLayout from '../../components/DocsLayout';
import AnchorHeading from '../../components/AnchorHeading';
import TerminalWindow from '../../components/TerminalWindow';
import { siteConfig } from '../../data/site';

const vmclockKernels: [string, string][] = [
  ['Amazon Linux 2', 'kernel-5.10.223-211.872.amzn2 and later'],
  ['Amazon Linux 2023', 'kernel-6.1.102-108.177.amzn2023 and later'],
  ['Upstream Linux', 'kernel 6.13 and later'],
];

export default function DocsPlatforms() {
  return (
    <DocsLayout
      title="Platforms"
      description="Supported platforms, kernel requirements for VMClock, and PHC setup on EC2."
    >
      <AnchorHeading level={2} id="supported">Supported platforms</AnchorHeading>
      <p>
        ClockBound runs on Linux on <code>x86_64</code> and <code>aarch64</code>{' '}
        (including AWS Graviton). Release artifacts include RPM packages for both
        architectures. For the most accurate bounds on AWS, run ClockBound on
        EC2 instances that have the{' '}
        <a
          href={siteConfig.enhancedAmazonTimeSyncServiceUrl}
          target="_blank"
          rel="noopener noreferrer"
        >enhanced Amazon Time Sync Service</a>{' '},
        and a <a href="#phc">configured PTP Hardware Clock</a>{' '}.
      </p>

      <AnchorHeading level={2} id="vmclock">VMClock &amp; kernel requirements</AnchorHeading>
      <p>
        The VMClock is a vDSO-style clock provided to VM guests. It lets
        user-space software such as ClockBound detect clock disruptions during
        VM maintenance events and take corrective action. When supported by the
        cloud provider and present, it appears at{' '}
        <code>/dev/vmclock0</code>.
      </p>
      <p>VMClock is included by default in:</p>
      <div className="overflow-x-auto">
        <table className="w-full text-left text-sm">
          <thead className="bg-base-700/60 text-slate-300">
            <tr>
              <th className="px-4 py-2 font-semibold">Distribution</th>
              <th className="px-4 py-2 font-semibold">Minimum kernel</th>
            </tr>
          </thead>
          <tbody>
            {vmclockKernels.map((row, i) => (
              <tr key={row[0]} className={i % 2 ? 'bg-base-800/40' : 'bg-base-800/10'}>
                <td className="px-4 py-2 font-medium text-white">{row[0]}</td>
                <td className="px-4 py-2 font-mono text-slate-400">{row[1]}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="mt-4">
        AWS is rolling out VMClock support on EC2 for Graviton, Intel, and AMD
        architectures. If <code>/dev/vmclock0</code> lacks the read permission
        ClockBound needs, grant it:
      </p>
      <TerminalWindow title="bash" code={`sudo chmod a+r /dev/vmclock0`} />

      <AnchorHeading level={2} id="phc">PTP Hardware Clock (PHC) on EC2</AnchorHeading>

      <p>The PTP Hardware Clock (PHC) on EC2 is available to EC2 instances
        that have the{' '}
        <a
          href={siteConfig.enhancedAmazonTimeSyncServiceUrl}
          target="_blank"
          rel="noopener noreferrer"
        >enhanced Amazon Time Sync Service</a>, and have the ENA driver
        installed and configured with PHC support.
      </p>

      <p>
        On <i>Amazon Linux</i>, the ENA driver is already installed.
        Assuming you are running on an EC2 instance that has support
        for the enhanced Amazon Time Sync Service, you can enable
        the ENA driver with PHC support by running the
        <code>configure_phc</code>{' '}
        script shipped in <code>clock-bound/assets</code>:
      </p>
      <TerminalWindow
        title="bash"
        code={`# Configure the PHC to be enabled on the next boot.
./clock-bound/assets/configure_phc -c

# Or alternatively ...

# Configure the PHC to be enabled immediately.
#
# WARNING: Running this reloads the EC2 instance's ENA driver,
# which causes temporary network packet loss.
./clock-bound/assets/configure_phc`}
      />

      <p>
        On <i>non-Amazon-Linux distributions</i>, the ENA driver will
        need to be installed and configured with PHC support manually.
        Assuming you are running on an EC2 instance that has support
        for the enhanced Amazon Time Sync Service, you can perform
        the setup for the PHC by following the steps in the {' '}
        <a
          href={siteConfig.enhancedAmazonTimeSyncServiceUrl}
          target="_blank"
          rel="noopener noreferrer">
        Access the PTP Hardware Clock (PHC)</a>{' '}
        section of the AWS EC2 documentation.
      </p>

    </DocsLayout>
  );
}
