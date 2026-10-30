import { useEffect, useState } from 'react';
import { stats as buildTimeStats } from '../data/site';

export interface RepoStats {
  stars: number | null;
  forks: number | null;
  downloads: number | null;
  contributors: number | null;
}

const GITHUB_REPO = 'aws/clock-bound';
const CRATE = 'clock-bound';
const FETCH_TIMEOUT_MS = 6000;

async function getJson(url: string): Promise<unknown | null> {
  // AbortController gives us a timeout so a slow/hanging third party never
  // blocks or spins indefinitely.
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort(), FETCH_TIMEOUT_MS);
  try {
    const res = await fetch(url, {
      signal: ctrl.signal,
      headers: { Accept: 'application/json' },
    });
    if (!res.ok) return null;
    return await res.json();
  } catch {
    // Network error, CORS, abort/timeout, or rate-limit — all non-fatal.
    return null;
  } finally {
    clearTimeout(timer);
  }
}

/**
 * Hybrid stats: returns the build-time baseline immediately (baked into the
 * pre-rendered HTML by scripts/fetch-stats.mjs), then attempts a live refresh
 * from the visitor's browser. On success the numbers update in place; on any
 * failure (rate limit, offline, CORS, timeout) the baseline is kept — the UI
 * never regresses to a placeholder or shows a spinner.
 */
export function useLiveStats(): RepoStats {
  const [stats, setStats] = useState<RepoStats>({
    stars: buildTimeStats.stars,
    forks: buildTimeStats.forks,
    downloads: buildTimeStats.downloads,
    contributors: buildTimeStats.contributors,
  });

  useEffect(() => {
    let cancelled = false;

    (async () => {
      const [repo, crate] = await Promise.all([
        getJson(`https://api.github.com/repos/${GITHUB_REPO}`),
        getJson(`https://crates.io/api/v1/crates/${CRATE}`),
      ]);
      if (cancelled) return;

      setStats((prev) => {
        const r = repo as { stargazers_count?: number; forks_count?: number } | null;
        const c = crate as { crate?: { downloads?: number } } | null;
        return {
          // Only overwrite a field when the live value is a real number;
          // otherwise fall back to the previous (build-time) value.
          stars: typeof r?.stargazers_count === 'number' ? r.stargazers_count : prev.stars,
          forks: typeof r?.forks_count === 'number' ? r.forks_count : prev.forks,
          downloads:
            typeof c?.crate?.downloads === 'number' ? c.crate.downloads : prev.downloads,
          contributors: prev.contributors, // not refreshed live (needs pagination)
        };
      });
    })();

    return () => {
      cancelled = true;
    };
  }, []);

  return stats;
}
