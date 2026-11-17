import type { ReactNode } from 'react';

interface DocTableProps {
  /** Column headers, in display order. */
  headers: string[];
  /** One array of cells per row, in header order. */
  rows: ReactNode[][];
}

// A docs reference table (fields, events, values), styled to match the
// segment layout table on the Protocol page. Scrolls horizontally on narrow
// screens instead of squeezing the columns.
export default function DocTable({ headers, rows }: DocTableProps) {
  return (
    <div className="my-4 overflow-x-auto">
      <table className="w-full text-left text-sm">
        <thead className="bg-base-700/60 text-slate-300">
          <tr>
            {headers.map((header) => (
              <th key={header} scope="col" className="px-4 py-2 font-semibold">
                {header}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, i) => (
            <tr key={i} className={i % 2 ? 'bg-base-800/40' : 'bg-base-800/10'}>
              {row.map((cell, j) => (
                <td
                  key={j}
                  className={`px-4 py-2 align-top ${
                    j === 0 ? 'font-medium text-white' : 'text-slate-400'
                  }`}
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
