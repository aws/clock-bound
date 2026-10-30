import { Link } from 'react-router-dom';
import {
  ArrowRight,
  Check,
  Github,
  Star,
  X as XIcon,
  BookOpen,
  GitPullRequest,
} from 'lucide-react';
import Badge from '../components/Badge';
import Button from '../components/Button';
import Card from '../components/Card';
import GridBackground from '../components/GridBackground';
import SectionHeading from '../components/SectionHeading';
import CodeTabs from '../components/CodeTabs';
import Icon from '../components/Icon';
import PointVsWindowDiagram from '../components/PointVsWindowDiagram';
import ArchitectureDiagram from '../components/ArchitectureDiagram';
import { siteConfig } from '../data/site';
import { formatCount } from '../lib/format';
import { useLiveStats } from '../lib/useLiveStats';
import { comparison, faqs, features, useCases } from '../data/content';
import type { Faq } from '../data/content';

/** Renders an FAQ answer, turning `item.link.text` (if present) into an in-site link. */
function FaqAnswer({ item }: { item: Faq }) {
  const idx = item.link ? item.a.indexOf(item.link.text) : -1;
  if (!item.link || idx < 0) return <>{item.a}</>;
  const end = idx + item.link.text.length;
  return (
    <>
      {item.a.slice(0, idx)}
      <Link
        to={item.link.to}
        className="text-accent-hover underline underline-offset-2 hover:text-white"
      >
        {item.link.text}
      </Link>
      {item.a.slice(end)}
    </>
  );
}

const quickstartTabs = [
  {
    label: 'Install daemon',
    title: 'bash',
    code: `# Install the RPM (RHEL / CentOS / Amazon Linux)
sudo rpm -i clockbound-*.rpm

# Enable and start the daemon
sudo systemctl enable clockbound
sudo systemctl start clockbound`,
  },
  {
    label: 'Rust client',
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
    label: 'C client',
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

export default function LandingPage() {
  // Build-time baseline, refreshed live in the browser when possible.
  const liveStats = useLiveStats();
  const statBar = [
    { label: 'crates.io downloads', value: formatCount(liveStats.downloads) },
    { label: 'GitHub Stars', value: formatCount(liveStats.stars) },
    { label: 'Contributors', value: formatCount(liveStats.contributors) },
    { label: 'Forks', value: formatCount(liveStats.forks) },
  ];

  return (
    <>
      {/* ===== Hero ===== */}
      <section className="relative overflow-hidden">
        <GridBackground />
        <div className="container-page py-20 text-center lg:py-28">
          <div className="animate-fade-in-up">
            <a
              href={siteConfig.githubUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="inline-block"
            >
              <Badge className="hover:border-accent/60">
                🎉 v3.0 is now live — Star us on GitHub
                <ArrowRight size={13} />
              </Badge>
            </a>
          </div>

          <h1 className="mx-auto mt-6 max-w-4xl text-balance text-4xl font-extrabold tracking-tight text-white sm:text-5xl lg:text-6xl">
            True time lives in a window.{' '}
            <span className="bg-accent-gradient bg-clip-text text-transparent">
              ClockBound hands you both edges.
            </span>
          </h1>

          <p className="mx-auto mt-6 max-w-2xl text-lg leading-relaxed text-slate-400">
            ClockBound is an open-source daemon and client library that gives
            every timestamp a reliable error bound — an{' '}
            <span className="font-mono text-cyan-glow">[earliest, latest]</span>{' '}
            window around reference time — so distributed systems can order events
            and reason about consistency with confidence.
          </p>

          <div className="mt-10 flex flex-col items-center justify-center gap-3 sm:flex-row">
            <Button href="/getting-started" size="lg">
              Get Started <ArrowRight size={18} />
            </Button>
            <Button
              href={siteConfig.githubUrl}
              external
              variant="secondary"
              size="lg"
            >
              <Github size={18} /> Star on GitHub
              {liveStats.stars !== null && (
                <span className="ml-1 inline-flex items-center gap-1 rounded border border-slate-600 px-1.5 py-0.5 text-xs text-slate-400">
                  <Star size={11} /> {formatCount(liveStats.stars)}
                </span>
              )}
            </Button>
          </div>
        </div>
      </section>

      {/* ===== Stats bar ===== */}
      {/* TODO(human): star/fork/download values are placeholders in data/site.ts */}
      <section className="border-y border-base-700/60 bg-base-800/40">
        <div className="container-page grid grid-cols-2 gap-6 py-8 sm:grid-cols-4">
          {statBar.map((s) => (
            <div key={s.label} className="text-center">
              <p className="font-mono text-2xl font-bold text-white sm:text-3xl">
                {s.value}
              </p>
              <p className="mt-1 text-sm text-slate-400">{s.label}</p>
            </div>
          ))}
        </div>
      </section>

      {/* ===== Problem narrative ===== */}
      <section className="container-page py-20 lg:py-28">
        <div className="grid items-center gap-12 lg:grid-cols-2">
          <div>
            <SectionHeading
              centered={false}
              id="problem"
              eyebrow="The problem"
              title="A timestamp is a point estimate with no error bars"
              description="Clocks drift. Reading one gives you a single number that looks authoritative but is quietly wrong by some unknown amount."
            />
            <div className="mt-6 space-y-4 text-slate-400">
              <p>
                Picture two database nodes in different regions, each stamping a
                write with its local clock. If one clock is 5&nbsp;ms fast and
                the other 5&nbsp;ms slow, their timestamps can imply the{' '}
                <em>wrong order</em>, and no amount of comparing single numbers
                will reveal it.
              </p>
              <p>
                ClockBound replaces the single number with a window that
                bounds reference time (UTC). When two windows don&apos;t
                overlap, the order is certain. When they do, your application
                knows it must not guess.
              </p>
            </div>
          </div>
          <PointVsWindowDiagram />
        </div>
      </section>

      {/* ===== Interactive quickstart ===== */}
      <section className="border-y border-base-700/60 bg-base-800/30">
        <div className="container-page py-20 lg:py-28">
          <SectionHeading
            id="quickstart"
            eyebrow="Quickstart"
            title="Running in a couple of commands"
            description="Install the daemon, then read bounded time from your language of choice."
          />
          <div className="mx-auto mt-10 max-w-3xl">
            <CodeTabs tabs={quickstartTabs} />
            <p className="mt-4 text-center text-sm text-slate-500">
              Prefer step-by-step?{' '}
              <Link
                to="/getting-started"
                className="text-accent-hover underline underline-offset-2 hover:text-white"
              >
                Read the full Getting Started guide
              </Link>
              .
            </p>
          </div>
        </div>
      </section>

      {/* ===== How it works ===== */}
      <section className="container-page py-20 lg:py-28">
        <div className="grid items-center gap-12 lg:grid-cols-2">
          <ArchitectureDiagram />
          <div className="lg:order-first">
            <SectionHeading
              centered={false}
              id="how-it-works"
              eyebrow="How it works"
              title="A daemon that bounds time, clients that read it for free"
            />
            <ul className="mt-6 space-y-4">
              {[
                'The daemon auto-detects NTP, PHC, and VMClock sources and keeps the system clock synchronized.',
                'It continuously computes the clock error bound and writes time + bound into a shared-memory segment.',
                'Clients read time, bound, and clock status from shared memory, then derive the earliest/latest window — no system call.',
                'A feed-forward sync algorithm decouples time calculation from the system clock for cleaner disruption handling.',
              ].map((step, i) => (
                <li key={i} className="flex gap-3">
                  <span className="mt-0.5 flex h-6 w-6 flex-none items-center justify-center rounded-full bg-accent/15 font-mono text-xs text-accent-hover">
                    {i + 1}
                  </span>
                  <span className="text-slate-300">{step}</span>
                </li>
              ))}
            </ul>
          </div>
        </div>
      </section>

      {/* ===== Features grid ===== */}
      <section className="border-t border-base-700/60 bg-base-800/30">
        <div className="container-page py-20 lg:py-28">
          <SectionHeading
            id="why-clockbound"
            eyebrow="Why ClockBound"
            title="Trustworthy time, without the footguns"
          />
          <div className="mt-12 grid gap-6 sm:grid-cols-2 lg:grid-cols-3">
            {features.map((f) => (
              <Card key={f.title} gradient>
                <div className="mb-4 inline-flex rounded-lg bg-accent/10 p-2.5 text-accent-hover">
                  <Icon name={f.icon} />
                </div>
                <h3 className="mb-2 text-lg font-semibold text-white">
                  {f.title}
                </h3>
                <p className="text-sm leading-relaxed text-slate-400">{f.body}</p>
              </Card>
            ))}
          </div>
        </div>
      </section>

      {/* ===== Use cases ===== */}
      <section className="container-page py-20 lg:py-28">
        <SectionHeading
          id="use-cases"
          eyebrow="Use cases"
          title="Where bounded time changes the game"
        />
        <div className="mt-12 grid gap-6 sm:grid-cols-2">
          {useCases.map((u) => (
            <Card key={u.title} gradient>
              <div className="mb-4 inline-flex rounded-lg bg-cyan-glow/10 p-2.5 text-cyan-glow">
                <Icon name={u.icon} />
              </div>
              <h3 className="mb-2 text-lg font-semibold text-white">{u.title}</h3>
              <p className="text-sm leading-relaxed text-slate-400">{u.body}</p>
            </Card>
          ))}
        </div>
      </section>

      {/* ===== Comparison ===== */}
      <section className="border-y border-base-700/60 bg-base-800/30">
        <div className="container-page py-20 lg:py-28">
          <SectionHeading
            id="comparison"
            eyebrow="The difference"
            title="clock_gettime() vs. ClockBound"
            description="Same timestamp you already read — now with the context you were missing."
          />
          <div className="mx-auto mt-10 max-w-2xl overflow-hidden rounded-2xl border border-base-600">
            <table className="w-full text-left text-sm">
              <thead className="bg-base-700/60 text-slate-300">
                <tr>
                  <th className="px-5 py-3 font-semibold">Capability</th>
                  <th className="px-5 py-3 text-center font-mono font-semibold">
                    clock_gettime()
                  </th>
                  <th className="px-5 py-3 text-center font-semibold text-accent-hover">
                    ClockBound
                  </th>
                </tr>
              </thead>
              <tbody>
                {comparison.map((row, i) => (
                  <tr
                    key={row.capability}
                    className={i % 2 ? 'bg-base-800/40' : 'bg-base-800/10'}
                  >
                    <td className="px-5 py-3 text-slate-300">{row.capability}</td>
                    <td className="px-5 py-3 text-center">
                      {row.naive ? (
                        <Check size={16} className="mx-auto text-emerald-glow" />
                      ) : (
                        <XIcon size={16} className="mx-auto text-slate-600" />
                      )}
                    </td>
                    <td className="px-5 py-3 text-center">
                      {row.clockbound ? (
                        <Check size={16} className="mx-auto text-emerald-glow" />
                      ) : (
                        <XIcon size={16} className="mx-auto text-slate-600" />
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      </section>

      {/* ===== FAQ ===== */}
      <section className="container-page py-20 lg:py-28">
        <SectionHeading id="faq" eyebrow="FAQ" title="Questions, answered" />
        <div className="mx-auto mt-10 max-w-3xl space-y-4">
          {faqs.map((item) => (
            <details
              key={item.q}
              className="group rounded-xl border border-base-600 bg-base-800/60 p-5 [&_summary]:cursor-pointer"
            >
              <summary className="flex items-center justify-between gap-4 font-medium text-white marker:content-['']">
                <span>{item.q}</span>
                <ArrowRight
                  size={16}
                  className="flex-none text-slate-500 transition-transform group-open:rotate-90"
                />
              </summary>
              <p className="mt-3 text-sm leading-relaxed text-slate-400">
                <FaqAnswer item={item} />
              </p>
            </details>
          ))}
        </div>
      </section>

      {/* ===== Community ===== */}
      <section className="border-t border-base-700/60 bg-base-800/30">
        <div className="container-page py-20 lg:py-28">
          <SectionHeading
            id="community"
            eyebrow="Community & ecosystem"
            title="Build with us"
            description="ClockBound is open source and welcomes contributions."
          />
          <div className="mx-auto mt-10 grid max-w-4xl gap-6 sm:grid-cols-2">
            {/* The whole card is the link, so the hover lift signals a real click target. */}
            <Link
              to="/docs/concepts"
              className="group block rounded-2xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
            >
              <Card hover className="text-center">
                <BookOpen className="mx-auto mb-3 text-accent-hover" />
                <h3 className="mb-1 font-semibold text-white">Documentation</h3>
                <p className="mb-4 text-sm text-slate-400">
                  Concepts, daemon, clients, and the shared-memory protocol.
                </p>
                <span className="text-sm text-accent-hover underline underline-offset-2 group-hover:text-white">
                  Read the docs
                </span>
              </Card>
            </Link>
            <a
              href={siteConfig.contributingUrl}
              target="_blank"
              rel="noopener noreferrer"
              className="group block rounded-2xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent"
            >
              <Card hover className="text-center">
                <GitPullRequest className="mx-auto mb-3 text-accent-hover" />
                <h3 className="mb-1 font-semibold text-white">Contribute</h3>
                <p className="mb-4 text-sm text-slate-400">
                  Issues, discussions, and pull requests on GitHub.
                </p>
                <span className="text-sm text-accent-hover underline underline-offset-2 group-hover:text-white">
                  Contributing guide
                </span>
              </Card>
            </a>
          </div>
        </div>
      </section>

      {/* ===== Final CTA ===== */}
      <section className="container-page py-20 text-center lg:py-28">
        <div className="gradient-border mx-auto max-w-3xl">
          <div className="rounded-2xl bg-base-800 px-6 py-14">
            <h2 className="text-3xl font-bold text-white sm:text-4xl">
              Stop trusting clocks blindly
            </h2>
            <p className="mx-auto mt-4 max-w-xl text-slate-400">
              Add reliable error bounds to your timestamps in minutes.
            </p>
            <div className="mt-8 flex flex-col items-center justify-center gap-3 sm:flex-row">
              <Button href="/getting-started" size="lg">
                Get Started <ArrowRight size={18} />
              </Button>
              <Button
                href={siteConfig.githubUrl}
                external
                variant="secondary"
                size="lg"
              >
                <Github size={18} /> View on GitHub
              </Button>
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
