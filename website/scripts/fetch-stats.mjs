// Build-time stats fetcher. Pulls live numbers from the GitHub REST API and
// crates.io and writes them to src/data/github-stats.json, which the site
// imports at build time. Any field that cannot be fetched (rate limit, offline,
// private repo, etc.) keeps its previous value from the existing JSON file, so
// a transient failure does not wipe out good data. Only if there is no previous
// value is the field written as null, and the UI falls back to a "—"
// placeholder. `fetchedAt` records the last time at least one field was
// freshly fetched.
//
// Runs in `prebuild`/`predev`. Node 18+ has a global fetch (Node 24 here).
//
// Optional: set GITHUB_TOKEN to raise the GitHub rate limit and to read the
// repo before it is public. Without it, unauthenticated calls still work once
// the repo is public (60 req/hr per IP, which is plenty for a build).
import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const outFile = resolve(here, '..', 'src', 'data', 'github-stats.json');

const REPO = 'aws/clock-bound';
const CRATE = 'clock-bound';
// crates.io (and good manners for GitHub) require a descriptive User-Agent.
const UA = 'clockbound-website-build (+https://github.com/aws/clock-bound)';
const TIMEOUT_MS = 10_000;

const ghHeaders = {
  'User-Agent': UA,
  Accept: 'application/vnd.github+json',
  'X-GitHub-Api-Version': '2022-11-28',
  ...(process.env.GITHUB_TOKEN
    ? { Authorization: `Bearer ${process.env.GITHUB_TOKEN}` }
    : {}),
};

async function getJson(url, headers) {
  const ctrl = new AbortController();
  const t = setTimeout(() => ctrl.abort(), TIMEOUT_MS);
  try {
    const res = await fetch(url, { headers, signal: ctrl.signal });
    if (!res.ok) {
      console.warn(`[fetch-stats] ${url} -> HTTP ${res.status} (skipping)`);
      return null;
    }
    return await res.json();
  } catch (err) {
    console.warn(`[fetch-stats] ${url} -> ${err.message} (skipping)`);
    return null;
  } finally {
    clearTimeout(t);
  }
}

// Count contributors via the paginated endpoint: request 1 per page and read
// the "last" page number from the Link header. Falls back to null on failure.
async function getContributorCount() {
  const url = `https://api.github.com/repos/${REPO}/contributors?per_page=1&anon=true`;
  const ctrl = new AbortController();
  const t = setTimeout(() => ctrl.abort(), TIMEOUT_MS);
  try {
    const res = await fetch(url, { headers: ghHeaders, signal: ctrl.signal });
    if (!res.ok) {
      console.warn(`[fetch-stats] ${url} -> HTTP ${res.status} (skipping)`);
      return null;
    }
    const link = res.headers.get('link');
    if (link) {
      const m = link.match(/[?&]page=(\d+)>;\s*rel="last"/);
      if (m) return Number(m[1]);
    }
    // No Link header => a single page; count the array length.
    const arr = await res.json();
    return Array.isArray(arr) ? arr.length : null;
  } catch (err) {
    console.warn(`[fetch-stats] ${url} -> ${err.message} (skipping)`);
    return null;
  } finally {
    clearTimeout(t);
  }
}

const repo = await getJson(`https://api.github.com/repos/${REPO}`, ghHeaders);
const crate = await getJson(`https://crates.io/api/v1/crates/${CRATE}`, {
  'User-Agent': UA,
});
const contributors = await getContributorCount();

// Previously written stats, used as the fallback for any field that could not
// be fetched this run. A missing or unparseable file just means no fallback.
function readPrevious() {
  try {
    return JSON.parse(readFileSync(outFile, 'utf8'));
  } catch {
    return {};
  }
}
const prev = readPrevious();

const fresh = {
  stars: repo?.stargazers_count ?? null,
  forks: repo?.forks_count ?? null,
  downloads: crate?.crate?.downloads ?? null,
  contributors: contributors ?? null,
};

const stats = {};
let anyFresh = false;
for (const [key, value] of Object.entries(fresh)) {
  if (value !== null) {
    stats[key] = value;
    anyFresh = true;
  } else {
    stats[key] = prev[key] ?? null;
    if (stats[key] !== null) {
      console.warn(`[fetch-stats] ${key}: fetch failed, keeping previous value ${stats[key]}`);
    }
  }
}
stats.fetchedAt = anyFresh ? new Date().toISOString() : (prev.fetchedAt ?? null);

writeFileSync(outFile, JSON.stringify(stats, null, 2) + '\n');
console.log('[fetch-stats] wrote', outFile, JSON.stringify(stats));
