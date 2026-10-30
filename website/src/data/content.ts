// Landing-page content, derived from the ClockBound repo (README.md, docs/,
// CHANGELOG.md). Kept as data so copy is easy to review and edit.

export const features = [
  {
    icon: 'Target',
    title: 'Bounded uncertainty',
    body: 'Every reading returns an [earliest, latest] interval that bounds UTC, plus a clock status — not a single number you have to trust blindly.',
  },
  {
    icon: 'Zap',
    title: 'Blazing-fast reads',
    body: 'Clients read time and its error bound straight from a shared-memory segment — no system call, no daemon round-trip. Benchmarked at ~10.6M reads/sec.',
  },
  {
    icon: 'Radio',
    title: 'Multiple time sources',
    body: 'Automatically discovers and blends NTP (Amazon Time Sync), a local PTP Hardware Clock (PHC), and the VMClock device for robust synchronization.',
  },
  {
    icon: 'ShieldAlert',
    title: 'Disruption-aware',
    body: 'Reads the VMClock to detect clock disruptions during VM maintenance and reflects them in the clock status, so applications avoid acting on untrustworthy time.',
  },
  {
    icon: 'Settings',
    title: 'Zero-config daemon',
    body: 'Batteries-included: the daemon auto-detects reference sources and platform, and runs on built-in defaults. An optional TOML config is there when you need it.',
  },
  {
    icon: 'Boxes',
    title: 'Clients in any language',
    body: 'First-party Rust crate and C/FFI library, plus a documented shared-memory protocol so you can write a client in any language that can mmap a file.',
  },
];

export const useCases = [
  {
    icon: 'Database',
    title: 'Distributed databases',
    body: 'Order conflicting writes across regions and resolve "which happened first" without a central coordinator, using bounded timestamps instead of hopeful ones.',
  },
  {
    icon: 'CandlestickChart',
    title: 'Financial markets & market data',
    body: 'Timestamp market data and orders with a bounded window, so exchanges and trading systems can reason about when information arrived at each node.',
  },
  {
    icon: 'ScrollText',
    title: 'Audit & forensic logs',
    body: 'Attach an explicit time window to each log entry so investigators know the real uncertainty on every timestamp, not a false point value.',
  },
  {
    icon: 'Network',
    title: 'Consensus & coordination',
    body: 'Feed commit-wait style protocols a trustworthy upper bound on clock error to shrink waits while preserving correctness.',
  },
  {
    icon: 'ArrowLeftRight',
    title: 'Event & transaction ordering',
    body: 'Decide consistent happened-before relationships between events produced on different nodes, independent of their geographic location.',
  },
];

export interface Faq {
  q: string;
  a: string;
  /** Optional in-site link: the first occurrence of `text` in `a` is rendered as a link to `to`. */
  link?: { text: string; to: string };
}

export const faqs: Faq[] = [
  {
    q: 'Do I have to run this on AWS?',
    a: 'No. ClockBound works with any NTP source, a local PTP Hardware Clock, or the VMClock device. It is tuned for EC2 (where you get the most accurate bounds with the Amazon Time Sync Service), but it is not AWS-only.',
  },
  {
    q: 'What is the performance overhead of reading time?',
    a: 'Reads come from a shared-memory segment with no system call or daemon round-trip. The bundled C example measured roughly 10.6 million calls per second on a single thread.',
  },
  {
    q: 'What platforms and kernels are supported?',
    a: 'Linux on x86_64 and aarch64 (including AWS Graviton). VMClock-based disruption support needs a recent kernel — Amazon Linux 2 kernel-5.10.223-211.872 and later, Amazon Linux 2023 kernel-6.1.102-108.177 and later, or upstream Linux 6.13+. See the Platforms page for details.',
    link: { text: 'Platforms page', to: '/docs/platforms' },
  },
  {
    q: 'How is this different from just calling clock_gettime()?',
    a: 'clock_gettime() hands you a single timestamp with no indication of how wrong it might be. ClockBound returns the same kind of timestamp wrapped in an error bound plus a synchronization status, so your application can reason about uncertainty instead of ignoring it.',
  },
  {
    q: 'What does it cost? What is the license?',
    a: 'ClockBound is free and open source, dual-licensed under MIT and Apache-2.0. Use it under either license, at your option.',
  },
];

// Side-by-side comparison for the "why not just clock_gettime" section.
export const comparison = [
  { capability: 'Returns current time', naive: true, clockbound: true },
  { capability: 'Error bound on the reading', naive: false, clockbound: true },
  { capability: 'Synchronization status', naive: false, clockbound: true },
  { capability: 'Clock-disruption awareness', naive: false, clockbound: true },
  { capability: 'Cross-node event ordering', naive: false, clockbound: true },
];

export const glossary = [
  {
    term: 'Clock Error Bound (CEB)',
    def: 'The maximum difference between the local clock and true (reference) time. ClockBound reports it as half the width of the [earliest, latest] window.',
  },
  {
    term: 'NTP',
    def: 'Network Time Protocol — synchronizes a computer clock to remote time servers over the network.',
  },
  {
    term: 'PHC',
    def: 'PTP Hardware Clock — a hardware clock (on EC2, exposed via the ENA driver) that provides a highly accurate local time reference.',
  },
  {
    term: 'VMClock',
    def: 'A vDSO-style clock exposed to VM guests (at /dev/vmclock0) that lets software detect clock disruptions caused by VM maintenance events.',
  },
  {
    term: 'TSC',
    def: 'Time Stamp Counter — a high-resolution CPU counter (rdtsc on x86, cntvct_el0 on aarch64) used to extrapolate time between daemon updates.',
  },
  {
    term: 'Clock status',
    def: 'The daemon state for the clock: Unknown (0), Synchronized (1), FreeRunning (2), or Disrupted (3).',
  },
];
