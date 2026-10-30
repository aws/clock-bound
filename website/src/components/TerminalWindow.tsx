import { useState } from 'react';
import { Check, Copy } from 'lucide-react';

interface TerminalWindowProps {
  /** The raw text copied to the clipboard (no highlighting markup). */
  code: string;
  /** Optional title shown in the window chrome (e.g. a file name or shell). */
  title?: string;
  /** Pre-rendered children for syntax-highlighted display. Falls back to code. */
  children?: React.ReactNode;
}

// A styled terminal/code preview window with a working copy-to-clipboard
// button. Uses the async Clipboard API with a legacy execCommand fallback so
// copy works even when served over plain HTTP (non-secure context).
export default function TerminalWindow({
  code,
  title = 'bash',
  children,
}: TerminalWindowProps) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(code);
      } else {
        const ta = document.createElement('textarea');
        ta.value = code;
        ta.style.position = 'fixed';
        ta.style.opacity = '0';
        document.body.appendChild(ta);
        ta.select();
        document.execCommand('copy');
        document.body.removeChild(ta);
      }
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Silent failure: copying is a convenience, the code is still visible.
    }
  };

  return (
    <div className="gradient-border overflow-hidden shadow-glow">
      <div className="overflow-hidden rounded-2xl bg-base-800">
        {/* Window chrome */}
        <div className="flex items-center justify-between border-b border-base-600 bg-base-700/60 px-4 py-2.5">
          <div className="flex items-center gap-2">
            <span className="h-3 w-3 rounded-full bg-red-400/80" />
            <span className="h-3 w-3 rounded-full bg-yellow-400/80" />
            <span className="h-3 w-3 rounded-full bg-emerald-400/80" />
            <span className="ml-3 font-mono text-xs text-slate-400">{title}</span>
          </div>
          <button
            type="button"
            onClick={copy}
            aria-label="Copy code to clipboard"
            className="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs text-slate-400 transition-colors hover:bg-base-600 hover:text-white"
          >
            {copied ? (
              <>
                <Check size={14} className="text-emerald-glow" /> Copied
              </>
            ) : (
              <>
                <Copy size={14} /> Copy
              </>
            )}
          </button>
        </div>
        {/* Code body */}
        <pre className="overflow-x-auto p-4 font-mono text-sm leading-relaxed text-slate-200">
          {children ?? <code>{code}</code>}
        </pre>
      </div>
    </div>
  );
}
