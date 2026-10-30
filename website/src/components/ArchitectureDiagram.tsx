// Inline SVG overview of the ClockBound architecture: time sources feed the
// daemon, which disciplines the system clock and publishes time + error bound
// into shared memory; clients read that shared memory with near-zero overhead.
//
// TODO(human/designer): functional first-draft diagram. Replace with a polished
// designed asset when available.
export default function ArchitectureDiagram() {
  return (
    <figure className="gradient-border">
      <div className="rounded-2xl bg-base-800 p-6">
        <svg
          viewBox="0 0 720 300"
          className="w-full"
          role="img"
          aria-label="ClockBound architecture. NTP, PHC, and VMClock time sources feed the ClockBound daemon. The daemon disciplines the system clock and writes the current as-of time and clock error bound into a shared memory segment. Client applications read that time and bound from shared memory and derive the earliest and latest window themselves."
        >
          {/* Time sources */}
          <g fontFamily="monospace" fontSize="14">
            <rect x="20" y="30" width="130" height="34" rx="6" fill="#111726" stroke="#334155" />
            <text x="85" y="52" textAnchor="middle" fill="#cbd5e1">NTP (Time Sync)</text>
            <rect x="20" y="130" width="130" height="34" rx="6" fill="#111726" stroke="#334155" />
            <text x="85" y="152" textAnchor="middle" fill="#cbd5e1">PHC device</text>
            <rect x="20" y="230" width="130" height="34" rx="6" fill="#111726" stroke="#334155" />
            <text x="85" y="252" textAnchor="middle" fill="#cbd5e1">VMClock</text>
          </g>

          {/* Arrows sources -> daemon */}
          <g stroke="#6366f1" strokeWidth="2" markerEnd="url(#arrow)">
            <line x1="150" y1="47" x2="270" y2="130" />
            <line x1="150" y1="147" x2="270" y2="150" />
            <line x1="150" y1="247" x2="270" y2="170" />
          </g>

          {/* Daemon */}
          <rect x="270" y="110" width="170" height="80" rx="8" fill="#1a2234" stroke="#6366f1" strokeWidth="2" />
          <text x="355" y="145" textAnchor="middle" fill="#ffffff" fontSize="17" fontFamily="Inter, sans-serif" fontWeight="600">
            clockbound
          </text>
          <text x="355" y="166" textAnchor="middle" fill="#94a3b8" fontSize="13" fontFamily="monospace">
            daemon
          </text>

          {/* Daemon -> shared memory */}
          <line x1="440" y1="150" x2="520" y2="150" stroke="#22d3ee" strokeWidth="2" markerEnd="url(#arrow)" />

          {/* Shared memory */}
          <rect x="520" y="110" width="180" height="80" rx="8" fill="#111726" stroke="#22d3ee" strokeWidth="2" />
          <text x="610" y="140" textAnchor="middle" fill="#ffffff" fontSize="15" fontFamily="Inter, sans-serif" fontWeight="600">
            shared memory
          </text>
          <text x="610" y="160" textAnchor="middle" fill="#94a3b8" fontSize="12" fontFamily="monospace">
            /var/run/clockbound
          </text>
          <text x="610" y="176" textAnchor="middle" fill="#22d3ee" fontSize="12" fontFamily="monospace">
            as-of time + bound
          </text>

          {/* Client reads time + bound and derives the window itself */}
          <rect x="505" y="226" width="210" height="58" rx="8" fill="#1a2234" stroke="#34d399" strokeWidth="2" />
          <text x="610" y="249" textAnchor="middle" fill="#ffffff" fontSize="14" fontFamily="Inter, sans-serif">
            your application (client)
          </text>
          <text x="610" y="269" textAnchor="middle" fill="#34d399" fontSize="12" fontFamily="monospace">
            derives [earliest, latest]
          </text>
          <line x1="610" y1="226" x2="610" y2="192" stroke="#34d399" strokeWidth="2" markerEnd="url(#arrow)" />

          <defs>
            <marker id="arrow" markerWidth="8" markerHeight="8" refX="6" refY="3" orient="auto">
              <path d="M0,0 L6,3 L0,6 Z" fill="currentColor" />
            </marker>
          </defs>
        </svg>
      </div>
      <figcaption className="px-6 pb-5 text-center text-sm text-slate-400">
        Time sources feed the daemon; the daemon publishes time + error bound to
        shared memory; clients read it with no system call and derive the
        earliest/latest window.
      </figcaption>
    </figure>
  );
}
