// Inline SVG illustrating the core idea: a naive timestamp is a single point
// (which may be wrong), while ClockBound returns a bounded [earliest, latest]
// window that bounds UTC.
//
// TODO(human/designer): this is a functional first-draft diagram. Replace with
// a polished designed asset (or an animation) when available.
export default function PointVsWindowDiagram() {
  return (
    <figure className="gradient-border">
      <div className="rounded-2xl bg-base-800 p-6">
        <svg
          viewBox="0 0 640 260"
          className="w-full"
          role="img"
          aria-label="Comparison of a single-point timestamp versus a bounded earliest-to-latest time window. The single point can fall away from UTC, while the bounded window brackets it."
        >
          <defs>
            <linearGradient id="win" x1="0" x2="1" y1="0" y2="0">
              <stop offset="0%" stopColor="#6366f1" stopOpacity="0.15" />
              <stop offset="50%" stopColor="#6366f1" stopOpacity="0.45" />
              <stop offset="100%" stopColor="#22d3ee" stopOpacity="0.15" />
            </linearGradient>
          </defs>

          {/* UTC reference line — intentionally off-center within the window
              to show reference time can lie anywhere inside the bounds, not at
              the midpoint. */}
          <line x1="362" y1="20" x2="362" y2="240" stroke="#34d399" strokeWidth="2" strokeDasharray="4 4" />
          <text x="362" y="14" textAnchor="middle" fill="#34d399" fontSize="14" fontFamily="monospace">
            UTC
          </text>

          {/* Naive timestamp row */}
          <text x="20" y="70" fill="#94a3b8" fontSize="15" fontFamily="Inter, sans-serif">
            clock_gettime()
          </text>
          <line x1="20" y1="90" x2="620" y2="90" stroke="#334155" strokeWidth="1" />
          {/* the single (wrong) point, offset from UTC */}
          <circle cx="250" cy="90" r="7" fill="#f87171" />
          <text x="242" y="118" textAnchor="middle" fill="#f87171" fontSize="13" fontFamily="monospace">
            a single point — may be wrong
          </text>

          {/* ClockBound row */}
          <text x="20" y="170" fill="#94a3b8" fontSize="15" fontFamily="Inter, sans-serif">
            ClockBound
          </text>
          <line x1="20" y1="190" x2="620" y2="190" stroke="#334155" strokeWidth="1" />
          {/* bounded window straddling UTC */}
          <rect x="250" y="178" width="150" height="24" rx="4" fill="url(#win)" stroke="#6366f1" />
          <line x1="250" y1="172" x2="250" y2="208" stroke="#818cf8" strokeWidth="2" />
          <line x1="400" y1="172" x2="400" y2="208" stroke="#22d3ee" strokeWidth="2" />
          <text x="250" y="226" textAnchor="middle" fill="#818cf8" fontSize="13" fontFamily="monospace">
            earliest
          </text>
          <text x="400" y="226" textAnchor="middle" fill="#22d3ee" fontSize="13" fontFamily="monospace">
            latest
          </text>
        </svg>
      </div>
      <figcaption className="px-6 pb-5 text-center text-sm text-slate-400">
        A naive timestamp is a single point that can silently drift from{' '}
        <span className="whitespace-nowrap">reference time (UTC)</span>.
        ClockBound returns a window that bounds it.
      </figcaption>
    </figure>
  );
}
