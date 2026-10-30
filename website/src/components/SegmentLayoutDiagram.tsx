// RFC-style "packet" diagram of the ClockBound v3 shared-memory
// segment. Reproduces the 32-bit-per-row ASCII layout from the upstream
// protocol reference as a crisp SVG, read top-to-bottom, 4 bytes per row.
//
// Driven entirely by src/data/segmentLayout.ts so it stays in sync with the
// field table below it.
//
// TODO(human/designer): first-draft functional diagram; refine styling when a
// designed asset is available.
import {
  segmentFields,
  categoryStyle,
  SEGMENT_SIZE_BYTES,
  type SegmentField,
} from '../data/segmentLayout';

const BYTES_PER_ROW = 4;
const CELL_W = 120; // px per byte column
const ROW_H = 40; // px per row
const PAD_X = 52; // left gutter for the byte-offset labels
const PAD_TOP = 34; // top gutter for the bit/byte ruler

const ROWS = SEGMENT_SIZE_BYTES / BYTES_PER_ROW; // 26
const GRID_W = BYTES_PER_ROW * CELL_W;
const WIDTH = PAD_X + GRID_W + 16;
const HEIGHT = PAD_TOP + ROWS * ROW_H + 8;

// A field may start/end mid-row, so we slice it into per-row rectangles.
interface Rect {
  field: SegmentField;
  x: number;
  y: number;
  w: number;
  h: number;
  /** show the label in this (the first/tallest) rect */
  primary: boolean;
}

function rectsForField(f: SegmentField): Rect[] {
  const rects: Rect[] = [];
  let pos = f.offset;
  const end = f.offset + f.size;
  let firstFullRow: Rect | null = null;

  while (pos < end) {
    const row = Math.floor(pos / BYTES_PER_ROW);
    const colInRow = pos % BYTES_PER_ROW;
    const rowEnd = (row + 1) * BYTES_PER_ROW;
    const sliceEnd = Math.min(end, rowEnd);
    const sliceBytes = sliceEnd - pos;

    const rect: Rect = {
      field: f,
      x: PAD_X + colInRow * CELL_W,
      y: PAD_TOP + row * ROW_H,
      w: sliceBytes * CELL_W,
      h: ROW_H,
      primary: false,
    };
    rects.push(rect);
    // Prefer to label the widest full-width slice (for tall multi-row fields),
    // falling back to the first slice.
    if (!firstFullRow || rect.w > firstFullRow.w) firstFullRow = rect;
    pos = sliceEnd;
  }
  (firstFullRow ?? rects[0]).primary = true;
  // Merge vertically-adjacent full-width slices into one tall box so multi-row
  // fields (8- and 16-byte) render as a single rectangle like the ASCII art.
  const allFullWidth = rects.every((r) => r.w === GRID_W);
  if (allFullWidth && rects.length > 1) {
    const merged: Rect = {
      field: f,
      x: rects[0].x,
      y: rects[0].y,
      w: GRID_W,
      h: rects.length * ROW_H,
      primary: true,
    };
    return [merged];
  }
  return rects;
}

// Approx. average glyph width for Inter at a given font size, used to decide
// whether a label fits a cell and, if not, how to wrap it onto two lines.
const AVG_GLYPH = 0.56;

function fitsWidth(text: string, fontSize: number, width: number): boolean {
  return text.length * fontSize * AVG_GLYPH <= width;
}

// Wrap a field name into at most two balanced lines so long names in narrow
// cells (e.g. "Period Error Shift" in a single byte) don't overflow.
function wrapName(name: string, fontSize: number, width: number): string[] {
  if (fitsWidth(name, fontSize, width)) return [name];
  const words = name.split(' ');
  if (words.length < 2) return [name];
  // Find the split point that best balances the two lines.
  let best: [string, string] | null = null;
  let bestDelta = Infinity;
  for (let i = 1; i < words.length; i++) {
    const a = words.slice(0, i).join(' ');
    const b = words.slice(i).join(' ');
    const delta = Math.abs(a.length - b.length);
    if (delta < bestDelta) {
      bestDelta = delta;
      best = [a, b];
    }
  }
  return best ? [best[0], best[1]] : [name];
}

export default function SegmentLayoutDiagram() {
  const rects = segmentFields.flatMap(rectsForField);

  return (
    <figure className="gradient-border not-prose my-6">
      <div className="overflow-x-auto rounded-2xl bg-base-800 p-5">
        <svg
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          className="h-auto w-full"
          role="img"
          aria-label="Packet-style diagram of the 104-byte ClockBound version 3 shared-memory segment, drawn four bytes per row from offset 0 to 104. Fields in order: Magic Number (8 bytes), Segment Size (4), Min Version (1), Max Version (1), Generation (2), As-Of TSC (8), As-Of Timestamp (16), Void-After Timestamp (16), Period (8), Period Error (8), Bound (8), Disruption Marker (8), Max Drift (4), Clock Status (4), Disruption Support (1), Period Shift (1), Period Error Shift (1), and 5 bytes of padding."
        >
          {/* Byte ruler across the top (0..3) */}
          {Array.from({ length: BYTES_PER_ROW }).map((_, b) => (
            <text
              key={`ruler-${b}`}
              x={PAD_X + b * CELL_W + CELL_W / 2}
              y={PAD_TOP - 12}
              textAnchor="middle"
              fill="#64748b"
              fontSize="11"
              fontFamily='"JetBrains Mono", monospace'
            >
              byte {b}
            </text>
          ))}

          {/* Per-row byte-offset labels down the left gutter */}
          {Array.from({ length: ROWS + 1 }).map((_, r) => (
            <text
              key={`off-${r}`}
              x={PAD_X - 8}
              y={PAD_TOP + r * ROW_H + 4}
              textAnchor="end"
              fill="#475569"
              fontSize="10"
              fontFamily='"JetBrains Mono", monospace'
            >
              {r * BYTES_PER_ROW}
            </text>
          ))}

          {/* Field rectangles */}
          {rects.map((r, i) => {
            const style = categoryStyle[r.field.category];
            return (
              <g key={`${r.field.name}-${i}`}>
                <rect
                  x={r.x + 1}
                  y={r.y + 1}
                  width={r.w - 2}
                  height={r.h - 2}
                  rx={4}
                  fill={style.fill}
                  stroke={style.stroke}
                  strokeWidth={1}
                />
                {r.primary && (() => {
                  const nameSize = r.field.name === 'Padding' ? 11 : 12.5;
                  const nameLines = wrapName(r.field.name, nameSize, r.w - 8);
                  const typeText =
                    r.field.type === '—'
                      ? `${r.field.size}B`
                      : `${r.field.type} · ${r.field.size}B`;
                  const lineH = 13;
                  // Total text block height: one line per name line + the type
                  // line. Center the whole block vertically in the rect.
                  const totalLines = nameLines.length + 1;
                  const blockTop =
                    r.y + r.h / 2 - ((totalLines - 1) * lineH) / 2;
                  return (
                    <>
                      {nameLines.map((line, li) => (
                        <text
                          key={li}
                          x={r.x + r.w / 2}
                          y={blockTop + li * lineH + 4}
                          textAnchor="middle"
                          fill="#e2e8f0"
                          fontSize={nameSize}
                          fontFamily="Inter, sans-serif"
                        >
                          {line}
                        </text>
                      ))}
                      <text
                        x={r.x + r.w / 2}
                        y={blockTop + nameLines.length * lineH + 4}
                        textAnchor="middle"
                        fill={style.text}
                        fontSize="10.5"
                        fontFamily='"JetBrains Mono", monospace'
                      >
                        {typeText}
                      </text>
                    </>
                  );
                })()}
              </g>
            );
          })}
        </svg>
      </div>
      <figcaption className="px-5 pb-4 pt-3 text-center text-sm text-slate-400">
        The 104-byte v3 segment, four bytes per row, read top to bottom. Colors
        group related fields.
      </figcaption>
    </figure>
  );
}
