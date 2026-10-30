// Single source of truth for the ClockBound v3 shared-memory segment layout.
// Byte offsets and sizes are taken from the upstream protocol reference:
// https://github.com/aws/clock-bound/blob/main/docs/protocol.md
//
// The v3 segment is 104 bytes. SegmentLayoutDiagram (RFC-style packet diagram)
// renders from this array, keeping the visualization in sync with the field
// table on the Protocol page.

export type SegmentCategory =
  | 'header' // magic / size / versioning / generation
  | 'tsc' // the TSC counter reference
  | 'time' // timestamps (as-of, void-after)
  | 'math' // period / period-error / bound used in the CEB calculation
  | 'status' // disruption marker, drift, clock status, flags
  | 'padding';

export interface SegmentField {
  name: string;
  /** Rust-ish type as shown in the field table. */
  type: string;
  /** Byte offset from the start of the segment. */
  offset: number;
  /** Size in bytes. */
  size: number;
  category: SegmentCategory;
}

export const SEGMENT_SIZE_BYTES = 104;

export const segmentFields: SegmentField[] = [
  { name: 'Magic Number', type: 'u64', offset: 0, size: 8, category: 'header' },
  { name: 'Segment Size', type: 'u32', offset: 8, size: 4, category: 'header' },
  { name: 'Min Version', type: 'u8', offset: 12, size: 1, category: 'header' },
  { name: 'Max Version', type: 'u8', offset: 13, size: 1, category: 'header' },
  { name: 'Generation', type: 'u16', offset: 14, size: 2, category: 'header' },
  { name: 'As-Of TSC', type: 'u64', offset: 16, size: 8, category: 'tsc' },
  { name: 'As-Of Timestamp', type: 'i64, i64', offset: 24, size: 16, category: 'time' },
  { name: 'Void-After Timestamp', type: 'i64, i64', offset: 40, size: 16, category: 'time' },
  { name: 'Period', type: 'u64', offset: 56, size: 8, category: 'math' },
  { name: 'Period Error', type: 'u64', offset: 64, size: 8, category: 'math' },
  { name: 'Bound', type: 'i64', offset: 72, size: 8, category: 'math' },
  { name: 'Disruption Marker', type: 'u64', offset: 80, size: 8, category: 'status' },
  { name: 'Max Drift', type: 'u32', offset: 88, size: 4, category: 'status' },
  { name: 'Clock Status', type: 'i32', offset: 92, size: 4, category: 'status' },
  { name: 'Disruption Support', type: 'u8', offset: 96, size: 1, category: 'status' },
  { name: 'Period Shift', type: 'u8', offset: 97, size: 1, category: 'status' },
  { name: 'Period Error Shift', type: 'u8', offset: 98, size: 1, category: 'status' },
  { name: 'Padding', type: '—', offset: 99, size: 5, category: 'padding' },
];

// Visual tints per category, drawn from the site palette (tailwind.config.js).
// fill is used for block backgrounds, stroke for borders, text for labels.
export const categoryStyle: Record<
  SegmentCategory,
  { label: string; fill: string; stroke: string; text: string }
> = {
  header: { label: 'Header & versioning', fill: 'rgba(99,102,241,0.18)', stroke: '#6366f1', text: '#818cf8' },
  tsc: { label: 'TSC reference', fill: 'rgba(34,211,238,0.14)', stroke: '#22d3ee', text: '#22d3ee' },
  time: { label: 'Timestamps', fill: 'rgba(52,211,153,0.14)', stroke: '#34d399', text: '#34d399' },
  math: { label: 'Period / bound math', fill: 'rgba(129,140,248,0.16)', stroke: '#818cf8', text: '#a5b4fc' },
  status: { label: 'Status & flags', fill: 'rgba(148,163,184,0.16)', stroke: '#94a3b8', text: '#cbd5e1' },
  padding: { label: 'Padding', fill: 'rgba(100,116,139,0.10)', stroke: '#475569', text: '#64748b' },
};

// Development-time invariant: fields must be contiguous and sum to the segment
// size. Throwing here surfaces any future edit that breaks the byte math.
if (import.meta.env?.DEV) {
  let cursor = 0;
  for (const f of segmentFields) {
    if (f.offset !== cursor) {
      throw new Error(
        `segmentLayout: field "${f.name}" offset ${f.offset} != expected ${cursor}`,
      );
    }
    cursor += f.size;
  }
  if (cursor !== SEGMENT_SIZE_BYTES) {
    throw new Error(`segmentLayout: total ${cursor} != ${SEGMENT_SIZE_BYTES}`);
  }
}
