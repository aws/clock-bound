import { Link } from 'react-router-dom';
import { AlertTriangle, ArrowRight, Terminal } from 'lucide-react';
import SectionHeading from '../components/SectionHeading';
import HeadingAnchor from '../components/HeadingAnchor';
import CodeTabs from '../components/CodeTabs';
import TerminalWindow from '../components/TerminalWindow';
import Button from '../components/Button';
import { siteConfig } from '../data/site';

const installTabs = [
  {
    label: 'RPM (release)',
    title: 'bash',
    code: `# Download the RPM from the GitHub releases page
# (x86_64 and aarch64 Linux packages are provided)
sudo rpm -i clockbound-*.rpm`,
  },
  {
    label: 'cargo install',
    title: 'bash',
    code: `# Build and install from crates.io. The daemon feature is required:
# without it, cargo builds the library but installs no binaries.
cargo install clock-bound --version "${siteConfig.version}" --features daemon

# Copy the binary into the path the systemd unit expects
sudo cp ~/.cargo/bin/clockbound /usr/bin/clockbound

# cargo install does NOT install the systemd unit (only the RPM does),
# so install it manually from the repo before enabling the service:
sudo curl -fsSL \\
  https://raw.githubusercontent.com/aws/clock-bound/main/clock-bound/assets/clockbound.service \\
  -o /usr/lib/systemd/system/clockbound.service
sudo systemctl daemon-reload`,
  },
];

const clientTabs = [
  {
    label: 'Rust',
    title: 'Cargo.toml',
    code: `[dependencies]
clock-bound = "${siteConfig.version}"`,
  },
  {
    label: 'Rust (usage)',
    title: 'main.rs',
    code: `use clock_bound::{Client, ClockStatus};

let mut client = Client::new()?;
let window = client.now()?;
match window.clock_status {
    ClockStatus::Synchronized | ClockStatus::FreeRunning => {
        println!("Time window: {:?} to {:?}", window.earliest, window.latest);
    }
    status => eprintln!("Time window not trustworthy, clock status: {status:?}"),
}`,
  },
  {
    label: 'C',
    title: 'clockbound_now.c',
    code: `#include <clockbound.h>

clockbound_now_result now;
clockbound_err cb_err;
clockbound_ctx *ctx = clockbound_open(&cb_err);

clockbound_now(ctx, &now, &cb_err);
if (now.clock_status == CLOCKBOUND_STA_SYNCHRONIZED ||
    now.clock_status == CLOCKBOUND_STA_FREE_RUNNING) {
    printf("Time window: %ld.%09ld to %ld.%09ld\\n",
           now.earliest.tv_sec, now.earliest.tv_nsec,
           now.latest.tv_sec, now.latest.tv_nsec);
} else {
    fprintf(stderr, "Time window not trustworthy, clock status: %d\\n", now.clock_status);
}`,
  },
];

const prerequisites = [
  ['OS / arch', 'Linux on x86_64 or aarch64 (incl. AWS Graviton)'],
  ['Privileges', 'The daemon runs as a privileged user (systemd service)'],
  [
    'VMClock (disruption support)',
    'AL2 kernel-5.10.223-211.872+, AL2023 kernel-6.1.102-108.177+, or upstream Linux 6.13+',
  ],
  ['C client', 'gcc and the libclockbound library + header installed'],
  [
    'Best accuracy',
    'EC2 instance with the enhanced Amazon Time Sync Service and a configured PTP Hardware Clock',
  ],
];

export default function GettingStartedPage() {
  return (
    <div className="container-page py-14 lg:py-20">
      <div className="mx-auto max-w-3xl">
        <SectionHeading
          centered={false}
          eyebrow="Getting Started"
          title="Install the daemon, read bounded time"
          description="ClockBound has two parts: the clockbound daemon that keeps the clock synchronized and publishes a bounded timestamp, and a client (Rust or C) that reads it. You need both."
        />

        {/* Step 1 */}
        <div className="mt-12">
          <h2 id="install" className="anchor-heading flex items-center gap-2 text-xl font-semibold text-white">
            <span className="flex h-7 w-7 items-center justify-center rounded-full bg-accent/15 font-mono text-sm text-accent-hover">
              1
            </span>
            Install &amp; start the daemon
            <HeadingAnchor id="install" />
          </h2>
          <p className="mt-3 text-slate-400">
            Install from a release RPM, or build from crates.io with{' '}
            <code className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-sm text-cyan-glow">
              cargo install
            </code>
            .
          </p>
          <div className="mt-5">
            <CodeTabs tabs={installTabs} />
          </div>
          <p className="mt-5 text-slate-400">
            Then enable and start it as a systemd service:
          </p>
          <div className="mt-3">
            <TerminalWindow
              title="bash"
              code={`sudo systemctl enable clockbound
sudo systemctl start clockbound`}
            />
          </div>
          <p className="mt-3 text-sm text-slate-500">
            The RPM installs the{' '}
            <code className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-sm text-cyan-glow">
              clockbound.service
            </code>{' '}
            systemd unit for you. If you installed with{' '}
            <code className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-sm text-cyan-glow">
              cargo install
            </code>
            , install the unit file first (shown in that tab) — otherwise{' '}
            <code className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-sm text-cyan-glow">
              systemctl enable clockbound
            </code>{' '}
            will report that the unit was not found.
          </p>
          <p className="mt-3 text-sm text-slate-500">
            The daemon is zero-config: it auto-detects reference time sources and
            the platform. For the optional TOML configuration, see the{' '}
            <Link
              to="/docs/daemon"
              className="text-accent-hover underline underline-offset-2 hover:text-white"
            >
              Daemon reference
            </Link>
            .
          </p>
        </div>

        {/* Step 2 */}
        <div className="mt-14">
          <h2 id="verify" className="anchor-heading flex items-center gap-2 text-xl font-semibold text-white">
            <span className="flex h-7 w-7 items-center justify-center rounded-full bg-accent/15 font-mono text-sm text-accent-hover">
              2
            </span>
            Verify it&apos;s synchronized
            <HeadingAnchor id="verify" />
          </h2>
          <p className="mt-3 text-slate-400">
            Check the clock status with <code className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-sm text-cyan-glow">cbctl</code>, the command-line utility that ships
            with the daemon:
          </p>
          <div className="mt-3">
            <TerminalWindow title="bash" code={`cbctl status`} />
          </div>
          <p className="mt-3 text-sm text-slate-500">
            For other subcommands, such as{' '}
            <code className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-sm text-cyan-glow">
              waitsync
            </code>{' '}
            and{' '}
            <code className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-sm text-cyan-glow">
              sources
            </code>
            , see the{' '}
            <Link
              to="/docs/cbctl"
              className="text-accent-hover underline underline-offset-2 hover:text-white"
            >
              <span className="font-mono">cbctl</span> reference
            </Link>
            .
          </p>
        </div>

        {/* Step 3 */}
        <div className="mt-14">
          <h2 id="add-client" className="anchor-heading flex items-center gap-2 text-xl font-semibold text-white">
            <span className="flex h-7 w-7 items-center justify-center rounded-full bg-accent/15 font-mono text-sm text-accent-hover">
              3
            </span>
            Add a client to your application
            <HeadingAnchor id="add-client" />
          </h2>
          <p className="mt-3 text-slate-400">
            Use the first-party Rust crate or the C/FFI library. Both read the
            daemon&apos;s shared-memory segment and return an{' '}
            <code className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-sm text-cyan-glow">
              earliest
            </code>
            /
            <code className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-sm text-cyan-glow">
              latest
            </code>{' '}
            pair plus a clock status.
          </p>
          <div className="mt-5">
            <CodeTabs tabs={clientTabs} />
          </div>
          <p className="mt-3 text-sm text-slate-500">
            Full build/link instructions for the C library live in the{' '}
            <Link
              to="/docs/clients"
              className="text-accent-hover underline underline-offset-2 hover:text-white"
            >
              Clients reference
            </Link>
            .
          </p>
        </div>

        {/* Prerequisites */}
        <div className="mt-14">
          <h2 id="prerequisites" className="anchor-heading flex items-center gap-2 text-xl font-semibold text-white">
            <Terminal size={20} className="text-accent-hover" />
            Prerequisites
            <HeadingAnchor id="prerequisites" />
          </h2>
          <div className="mt-4 overflow-hidden rounded-2xl border border-base-600">
            <table className="w-full text-left text-sm">
              <tbody>
                {prerequisites.map(([k, v], i) => (
                  <tr
                    key={k}
                    className={i % 2 ? 'bg-base-800/40' : 'bg-base-800/10'}
                  >
                    <th className="w-1/3 px-5 py-3 align-top font-medium text-white">
                      {k}
                    </th>
                    <td className="px-5 py-3 text-slate-400">{v}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="mt-5 flex gap-3 rounded-xl border border-yellow-500/30 bg-yellow-500/5 p-4">
            <AlertTriangle size={18} className="mt-0.5 flex-none text-yellow-400" />
            <p className="text-sm text-slate-300">
              If <code className="font-mono text-cyan-glow">/dev/vmclock0</code>{' '}
              exists but the daemon can&apos;t read it, grant read access:{' '}
              <code className="font-mono text-cyan-glow">
                sudo chmod a+r /dev/vmclock0
              </code>
              . See{' '}
              <Link
                to="/docs/platforms"
                className="text-accent-hover underline underline-offset-2 hover:text-white"
              >
                Platforms
              </Link>{' '}
              for PHC and EC2 setup.
            </p>
          </div>
        </div>

        {/* Next */}
        <div className="mt-14 flex flex-col gap-3 border-t border-base-700 pt-8 sm:flex-row">
          <Button href="/docs/concepts" variant="secondary">
            Learn the concepts <ArrowRight size={16} />
          </Button>
          <Button href="/docs/protocol" variant="ghost">
            Build a custom client <ArrowRight size={16} />
          </Button>
        </div>
      </div>
    </div>
  );
}
