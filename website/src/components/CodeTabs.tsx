import { useState } from 'react';
import TerminalWindow from './TerminalWindow';

export interface CodeTab {
  label: string;
  /** Window chrome title, e.g. 'bash', 'Cargo.toml', 'client.rs'. */
  title?: string;
  code: string;
  /** Optional highlighted rendering; falls back to plain code. */
  rendered?: React.ReactNode;
}

interface CodeTabsProps {
  tabs: CodeTab[];
}

// Tabbed set of terminal/code windows. Each tab carries its own copy button
// via the underlying TerminalWindow.
export default function CodeTabs({ tabs }: CodeTabsProps) {
  const [active, setActive] = useState(0);
  const current = tabs[active];

  return (
    <div>
      <div
        role="tablist"
        aria-label="Code examples"
        className="mb-3 flex flex-wrap gap-1 rounded-lg border border-base-600 bg-base-800/60 p-1"
      >
        {tabs.map((tab, i) => (
          <button
            key={tab.label}
            role="tab"
            aria-selected={i === active}
            onClick={() => setActive(i)}
            className={`rounded-md px-3 py-1.5 text-sm font-medium transition-colors ${
              i === active
                ? 'bg-accent text-white'
                : 'text-slate-400 hover:bg-base-700 hover:text-white'
            }`}
          >
            {tab.label}
          </button>
        ))}
      </div>
      <TerminalWindow code={current.code} title={current.title ?? current.label}>
        {current.rendered}
      </TerminalWindow>
    </div>
  );
}
