import type { ReactNode } from 'react';

interface BadgeProps {
  children: ReactNode;
  className?: string;
}

// Small pill used for the announcement badge and inline tags.
export default function Badge({ children, className = '' }: BadgeProps) {
  return (
    <span
      className={`inline-flex items-center gap-2 rounded-full border border-accent/30 bg-accent/10 px-3 py-1 text-xs font-medium text-accent-hover ${className}`}
    >
      {children}
    </span>
  );
}
