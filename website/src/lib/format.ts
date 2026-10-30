// Formats a raw count for compact display in the stats bar.
// null/undefined -> "—" (unknown; e.g. a fetch failed or repo not yet public).
// 1234 -> "1.2k", 1_200_000 -> "1.2M", small numbers shown as-is.
export function formatCount(n: number | null | undefined): string {
  if (n === null || n === undefined || Number.isNaN(n)) return '—';
  if (n < 1000) return String(n);
  if (n < 1_000_000) {
    const v = n / 1000;
    return `${v >= 100 ? Math.round(v) : v.toFixed(1).replace(/\.0$/, '')}k`;
  }
  const v = n / 1_000_000;
  return `${v >= 100 ? Math.round(v) : v.toFixed(1).replace(/\.0$/, '')}M`;
}
