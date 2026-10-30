// Central site configuration. Edit real URLs, version numbers, and stats here.
// Items marked TODO must be verified/supplied by a human before launch.

// Live social-proof metrics are fetched at build time by
// scripts/fetch-stats.mjs (GitHub REST API + crates.io) into
// github-stats.json. Any field that could not be fetched is null here and the
// UI renders a "—" placeholder instead of a wrong number.
import githubStats from './github-stats.json';

export const stats = githubStats as {
  stars: number | null;
  forks: number | null;
  downloads: number | null;
  contributors: number | null;
  fetchedAt: string | null;
};

export const siteConfig = {
  name: 'ClockBound',
  version: '3.0.0-rc.0',

  // Real, known-good URLs
  githubUrl: 'https://github.com/aws/clock-bound',
  cratesUrl: 'https://crates.io/crates/clock-bound',
  contributingUrl: 'https://github.com/aws/clock-bound/blob/main/CONTRIBUTING.md',
  issuesUrl: 'https://github.com/aws/clock-bound/issues',
  amazonTimeSyncUrl:
    'https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/set-time.html',
  enhancedAmazonTimeSyncServiceUrl:
    'https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configure-ec2-ntp.html#enhanced-amazon-time-sync-service',
  accessThePTPHardwareClockUrl:
    'https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/configure-ec2-ntp.html#connect-to-the-ptp-hardware-clock',

  // Real performance figure: from docs/clockbound-ffi.md sample output,
  // 100,000,000 calls in 9.43s => 10,606,335 tps. Rounded for display.
  readsPerSecond: '10.6M',

  license: 'MIT OR Apache-2.0',
  copyrightHolder: 'Amazon Web Services, Inc. or its affiliates',
};

export const navLinks = [
  { label: 'Getting Started', href: '/getting-started' },
  { label: 'Docs', href: '/docs/concepts' },
];

export const docsNav = [
  { label: 'Concepts', href: '/docs/concepts' },
  { label: 'Daemon', href: '/docs/daemon' },
  { label: 'cbctl', href: '/docs/cbctl' },
  { label: 'Clients', href: '/docs/clients' },
  { label: 'Protocol', href: '/docs/protocol' },
  { label: 'Platforms', href: '/docs/platforms' },
];
