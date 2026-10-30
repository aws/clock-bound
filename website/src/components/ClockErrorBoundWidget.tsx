import { useEffect, useRef, useState } from 'react';

// ─────────────────────────────────────────────────────────────────────────────
// Clock Error Bound widget: "inspect cursor"
//
// A fixed sawtooth error-bound band is drawn across perfect time. The user drags
// a vertical "now" cursor left/right to inspect the window of uncertainty at any
// instant: the band's earliest/latest edges at that time, plus the true-time dot.
//
// The band grows linearly between periodic clock updates and snaps back to a
// narrow width at each update with a VERTICAL edge (the physical behaviour — the
// bound collapses instantaneously when a fresh reference measurement arrives —
// correcting the slanted edge in the static PNG above).
//
// This is a schematic illustration, not a simulation of real ppb/dispersion.
// ─────────────────────────────────────────────────────────────────────────────

// viewBox geometry
const W = 640;
const H = 380;
const PAD_L = 48; // left padding (y-axis)
const PAD_R = 24;
const PAD_T = 24;
const PAD_B = 44; // bottom padding (x-axis)

const PLOT_L = PAD_L;
const PLOT_R = W - PAD_R;
const PLOT_T = PAD_T;
const PLOT_B = H - PAD_B;
const PLOT_W = PLOT_R - PLOT_L;

// Model: N update intervals across the plot. Between updates the half-width of
// the bound grows linearly from BASE at a fixed DRIFT (in y-units per x-unit).
const UPDATES = 4; // number of clock-update resets across the width
const SEG_W = PLOT_W / UPDATES; // x-width of one interval
const BASE = 10; // half-width just after an update (y units)
const DRIFT = 0.28; // growth of half-width per x unit
// The "perfect synchronization" reference line (straight) — what the clock
// would read with zero error. The reported line weaves around this.
const LINE_Y0 = PLOT_B - 82; // perfect line y at t=0
const LINE_SLOPE = -0.231; // y units per x unit (negative = upward on screen)
// How far the reported line drifts away from perfect within one interval.
const WEAVE = 26; // default peak deviation (y units) reached just before an update
// Per-interval signed peak deviation of the midpoint from the perfect line
// (negative = upward on screen). Intervals 0 and 2 tilt down less (flatter,
// closer to perfect sync) so their cones point slightly more upward.
const SEG_WEAVE = [10, -WEAVE, 10, -WEAVE];
// Small residual error (y px) left at each clock update — the corrected clock
// is close to, but not exactly on, reference time. Signed, varied per update.
const UPDATE_OFFSET = [-7, 5, -4];
/** Signed peak deviation for an interval (falls back to alternating default). */
function segWeave(seg: number): number {
  return SEG_WEAVE[seg] ?? (seg % 2 === 0 ? WEAVE : -WEAVE);
}
/** Residual offset (y px) at the START of an interval = the update dot it
 *  begins from. Interval 0 starts at the chart origin (no update, offset 0);
 *  interval seg (>=1) starts at update dot (seg-1). */
function startOffset(seg: number): number {
  return seg === 0 ? 0 : UPDATE_OFFSET[seg - 1] ?? 0;
}

/** The straight "perfect synchronization" line at a given x pixel. */
function perfectY(x: number): number {
  return LINE_Y0 + LINE_SLOPE * (x - PLOT_L);
}

/** Reported clock value (center of the band) — weaves around the perfect line,
 *  drifting away within each interval and correcting (crossing) at each update. */
function centerY(x: number): number {
  const seg = Math.min(UPDATES - 1, Math.floor((x - PLOT_L) / SEG_W));
  const into = (x - PLOT_L) - seg * SEG_W; // distance since last update
  const t = into / SEG_W; // 0..1 within the interval
  // Start at the update dot (perfect + residual), fade the residual out as the
  // weave drifts to the interval's peak.
  return perfectY(x) + startOffset(seg) * (1 - t) + segWeave(seg) * t;
}

/** Half-width of the error bound at a given x pixel (sawtooth). */
function halfWidth(x: number): number {
  // Use the clamped segment so the final sample at x = PLOT_R stays at the END
  // of the last interval (full width) instead of wrapping to 0 (a phantom
  // update at the right edge).
  const seg = segIndex(x);
  const into = x - PLOT_L - seg * SEG_W; // distance since last update
  return BASE + DRIFT * into;
}

/** Index of the most recent update at/before x. */
function segIndex(x: number): number {
  return Math.min(UPDATES - 1, Math.floor((x - PLOT_L) / SEG_W));
}

export default function ClockErrorBoundWidget() {
  const svgRef = useRef<SVGSVGElement | null>(null);
  // Cursor position as a fraction [0,1] of the plot width. Start mid-interval.
  const [frac, setFrac] = useState(0.42);
  const [dragging, setDragging] = useState(false);

  const cursorX = PLOT_L + frac * PLOT_W;
  const cY = centerY(cursorX);
  const hw = halfWidth(cursorX);
  const earliestY = cY + hw;
  const latestY = cY - hw;
  // Current bound half-width for the live readout (schematic units shown as µsec).
  const boundIndicator = Math.round(hw);

  // Convert a client pointer x to a plot fraction.
  const pointerToFrac = (clientX: number): number => {
    const svg = svgRef.current;
    if (!svg) return frac;
    const rect = svg.getBoundingClientRect();
    // Map screen x into viewBox x, then into plot fraction.
    const vbX = ((clientX - rect.left) / rect.width) * W;
    const f = (vbX - PLOT_L) / PLOT_W;
    return Math.max(0, Math.min(1, f));
  };

  useEffect(() => {
    if (!dragging) return;
    const move = (e: MouseEvent | TouchEvent) => {
      const clientX =
        'touches' in e ? e.touches[0]?.clientX ?? 0 : (e as MouseEvent).clientX;
      setFrac(pointerToFrac(clientX));
    };
    const up = () => setDragging(false);
    window.addEventListener('mousemove', move);
    window.addEventListener('mouseup', up);
    window.addEventListener('touchmove', move, { passive: false });
    window.addEventListener('touchend', up);
    return () => {
      window.removeEventListener('mousemove', move);
      window.removeEventListener('mouseup', up);
      window.removeEventListener('touchmove', move);
      window.removeEventListener('touchend', up);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [dragging]);

  // Keyboard accessibility: arrow keys nudge the cursor.
  const onKeyDown = (e: React.KeyboardEvent) => {
    const step = e.shiftKey ? 0.1 : 0.02;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowDown') {
      setFrac((f) => Math.max(0, f - step));
      e.preventDefault();
    } else if (e.key === 'ArrowRight' || e.key === 'ArrowUp') {
      setFrac((f) => Math.min(1, f + step));
      e.preventDefault();
    } else if (e.key === 'Home') {
      setFrac(0);
      e.preventDefault();
    } else if (e.key === 'End') {
      setFrac(1);
      e.preventDefault();
    }
  };

  // Build the band (top = latest edge, bottom = earliest edge) and the weaving
  // center line by walking each interval explicitly, so the VERTICAL reset lands
  // EXACTLY on the update boundary (x = PLOT_L + seg*SEG_W) and coincides with
  // the dotted update line (no spill past it).
  const topPts: string[] = [];
  const botPts: string[] = [];
  // Center line is built as one polyline PER interval so it is NOT connected
  // across update boundaries — this avoids drawing a vertical segment when the
  // midpoint is corrected at each update.
  const centerSegs: string[][] = [];
  const PER_SEG = 60; // samples within one interval

  for (let seg = 0; seg < UPDATES; seg++) {
    const x0 = PLOT_L + seg * SEG_W; // this update boundary (narrow start)
    const peak = segWeave(seg);
    const off = startOffset(seg); // residual at the interval's start (the dot)
    const segPts: string[] = [];
    for (let j = 0; j <= PER_SEG; j++) {
      const t = j / PER_SEG; // 0..1 within interval
      const x = x0 + t * SEG_W;
      // Start at the update dot (perfect + residual), fade the residual out as
      // the weave drifts in, so the cone + line begin shifted by the same amount
      // as the dot and settle onto the weave by the interval's end.
      const cy = perfectY(x) + off * (1 - t) + peak * t;
      const h = BASE + DRIFT * (t * SEG_W); // widening half-width
      topPts.push(`${x.toFixed(2)},${(cy - h).toFixed(2)}`);
      botPts.push(`${x.toFixed(2)},${(cy + h).toFixed(2)}`);
      segPts.push(`${x.toFixed(2)},${cy.toFixed(2)}`);
    }
    centerSegs.push(segPts);
    // Band edges snap vertically exactly on the next boundary; the narrow start
    // lands at the NEXT interval's dot (perfect + its residual offset).
    if (seg < UPDATES - 1) {
      const xb = PLOT_L + (seg + 1) * SEG_W;
      const cyNarrow = perfectY(xb) + startOffset(seg + 1);
      topPts.push(`${xb.toFixed(2)},${(cyNarrow - BASE).toFixed(2)}`);
      botPts.push(`${xb.toFixed(2)},${(cyNarrow + BASE).toFixed(2)}`);
    }
  }
  // Polygon path: top edge left→right, then bottom edge right→left.
  const bandPath = `M ${topPts.join(' L ')} L ${botPts.slice().reverse().join(' L ')} Z`;
  // Reported (weaving) center line — one sub-path per interval, joined into a
  // single path string with separate M (move) commands so the segments are not
  // connected across updates (no vertical jump).
  const reportedPath = centerSegs
    .map((seg) => `M ${seg.join(' L ')}`)
    .join(' ');
  // Perfect synchronization reference line (straight).
  const perfectPath = `M ${PLOT_L},${perfectY(PLOT_L).toFixed(1)} L ${PLOT_R},${perfectY(PLOT_R).toFixed(1)}`;

  // Update marker x positions (skip t=0).
  const updateXs = Array.from({ length: UPDATES }, (_, i) => PLOT_L + i * SEG_W).filter(
    (_, i) => i > 0,
  );

  const pct = Math.round(frac * 100);

  return (
    <figure className="gradient-border not-prose my-6">
      <div className="rounded-2xl bg-base-800 p-5">
        <div className="mb-3 flex items-center justify-between">
          <span className="text-sm font-semibold text-white">
            Explore the window of uncertainty
          </span>
          <span className="rounded bg-base-700 px-2 py-0.5 font-mono text-[11px] text-slate-400">
            drag the cursor
          </span>
        </div>

        {/* Legend: explains the chart markers */}
        <div className="mb-2 flex flex-wrap items-center gap-x-5 gap-y-1 text-[11px] text-slate-400">
          <span className="inline-flex items-center gap-1.5">
            <svg width="26" height="10" aria-hidden="true">
              <line x1="0" y1="5" x2="26" y2="5" stroke="#64748b" strokeDasharray="3 4" strokeWidth="1.5" />
              <circle cx="13" cy="5" r="3" fill="#fff" stroke="#0f172a" strokeWidth="1" />
            </svg>
            clock update
          </span>
          <span className="inline-flex items-center gap-1.5">
            <svg width="14" height="12" aria-hidden="true">
              <line x1="7" y1="1" x2="7" y2="11" stroke="#f472b6" strokeWidth="2" />
              <line x1="2" y1="1" x2="12" y2="1" stroke="#f472b6" strokeWidth="2" />
              <line x1="2" y1="11" x2="12" y2="11" stroke="#f472b6" strokeWidth="2" />
            </svg>
            earliest / latest
          </span>
          <span className="inline-flex items-center gap-1.5">
            <svg width="26" height="10" aria-hidden="true">
              <line x1="0" y1="5" x2="26" y2="5" stroke="#2dd4bf" strokeWidth="2" />
            </svg>
            midpoint of earliest / latest
          </span>
          <span className="inline-flex items-center gap-1.5">
            <svg width="26" height="10" aria-hidden="true">
              <line x1="0" y1="5" x2="26" y2="5" stroke="#9333ea" strokeWidth="2" strokeDasharray="6 5" />
            </svg>
            perfect synchronization
          </span>
        </div>

        <svg
          ref={svgRef}
          viewBox={`0 0 ${W} ${H}`}
          className="w-full touch-none select-none"
          role="img"
          aria-label="Interactive chart of the clock error bound over perfect time. A sawtooth band widens between periodic clock updates and snaps back to a narrow width at each update. Drag the vertical cursor to inspect the earliest and latest bounds at any instant."
        >
          <defs>
            <linearGradient id="cebBandA" x1="0" y1="0" x2="1" y2="0">
              <stop offset="0%" stopColor="#6366f1" stopOpacity="0.35" />
              <stop offset="100%" stopColor="#22d3ee" stopOpacity="0.35" />
            </linearGradient>
          </defs>

          {/* Axes */}
          <line x1={PLOT_L} y1={PLOT_T} x2={PLOT_L} y2={PLOT_B} stroke="#475569" strokeWidth="1.5" />
          <line x1={PLOT_L} y1={PLOT_B} x2={PLOT_R} y2={PLOT_B} stroke="#475569" strokeWidth="1.5" />
          <text x={20} y={(PLOT_T + PLOT_B) / 2} fill="#94a3b8" fontSize="12" fontFamily="Inter, sans-serif" textAnchor="middle" transform={`rotate(-90 20 ${(PLOT_T + PLOT_B) / 2})`}>
            ClockBound time, C(t)
          </text>
          <text x={(PLOT_L + PLOT_R) / 2} y={H - 10} fill="#94a3b8" fontSize="12" fontFamily="Inter, sans-serif" textAnchor="middle">
            reference time, t
          </text>

          {/* Error-bound band */}
          <path d={bandPath} fill="url(#cebBandA)" stroke="#6366f1" strokeWidth="1" strokeOpacity="0.5" />

          {/* Perfect synchronization reference line (straight, dashed) */}
          <path d={perfectPath} fill="none" stroke="#9333ea" strokeWidth="2" strokeDasharray="6 5" />

          {/* Reported clock line (center of band) — weaves around perfect sync */}
          <path d={reportedPath} fill="none" stroke="#2dd4bf" strokeWidth="2" />

          {/* Update markers: vertical ticks + dots. Identified via the legend above. */}
          {updateXs.map((ux, i) => (
            <g key={ux}>
              <line x1={ux} y1={PLOT_T} x2={ux} y2={PLOT_B} stroke="#64748b" strokeDasharray="3 4" strokeWidth="1" strokeOpacity="0.5" />
              <circle cx={ux} cy={perfectY(ux) + (UPDATE_OFFSET[i] ?? 0)} r="4" fill="#fff" stroke="#0f172a" strokeWidth="1.5" />
            </g>
          ))}

          {/* Cursor: earliest/latest bracket at the dragged instant */}
          <line x1={cursorX} y1={latestY} x2={cursorX} y2={earliestY} stroke="#f472b6" strokeWidth="2.5" />
          {/* bracket caps */}
          <line x1={cursorX - 7} y1={latestY} x2={cursorX + 7} y2={latestY} stroke="#f472b6" strokeWidth="2.5" />
          <line x1={cursorX - 7} y1={earliestY} x2={cursorX + 7} y2={earliestY} stroke="#f472b6" strokeWidth="2.5" />
          {/* true-time dot sits on the perfect-synchronization line, inside the band */}
          <circle cx={cursorX} cy={perfectY(cursorX)} r="4.5" fill="#f472b6" />

          {/* labels near cursor */}
          <text x={cursorX + 10} y={latestY - 4} fill="#f9a8d4" fontSize="11" fontFamily="monospace">latest</text>
          <text x={cursorX + 10} y={earliestY + 12} fill="#f9a8d4" fontSize="11" fontFamily="monospace">earliest</text>

          {/* Draggable handle on the x-axis */}
          <g
            tabIndex={0}
            role="slider"
            aria-label="Now cursor position across perfect time"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={pct}
            aria-valuetext={`${pct}% across the timeline`}
            onKeyDown={onKeyDown}
            onMouseDown={() => setDragging(true)}
            onTouchStart={() => setDragging(true)}
            style={{ cursor: 'ew-resize', outline: 'none' }}
          >
            {/* larger invisible hit area */}
            <rect x={cursorX - 14} y={PLOT_T} width={28} height={PLOT_B - PLOT_T + 18} fill="transparent" />
            <circle cx={cursorX} cy={PLOT_B + 12} r="7" fill="#f472b6" stroke="#0f172a" strokeWidth="1.5" />
            <path d={`M ${cursorX - 4} ${PLOT_B + 12} l 8 0 M ${cursorX} ${PLOT_B + 8} l 0 8`} stroke="#0f172a" strokeWidth="1.2" />
          </g>
        </svg>

        {/* Live readout */}
        <div className="mt-3 flex flex-wrap items-center justify-center gap-x-6 gap-y-1 text-center text-xs text-slate-400">
          <span>
            Current <span className="text-slate-200">Clock Error Bound</span>:{' '}
            <span className="font-mono text-pink-300">{boundIndicator}{' '}&#956;sec</span>
          </span>
        </div>
        <p className="mt-2 text-center text-xs text-slate-400">
          Drag right to advance time: the bound grows as the clock drifts, then
          snaps back at each clock update. Schematic illustration.
        </p>
      </div>
    </figure>
  );
}
