import type { ReactNode } from 'react';

interface CardProps {
  children: ReactNode;
  className?: string;
  /** When true, wraps the card in a glowing gradient border. */
  gradient?: boolean;
  /** Adds an interactive lift + glow on hover. */
  hover?: boolean;
}

export default function Card({
  children,
  className = '',
  gradient = false,
  hover = false,
}: CardProps) {
  const inner = (
    <div
      className={`h-full rounded-2xl bg-base-800 p-6 ${
        hover
          ? 'transition-all duration-200 hover:-translate-y-1 hover:shadow-glow'
          : ''
      } ${className}`}
    >
      {children}
    </div>
  );

  if (gradient) {
    return <div className="gradient-border h-full">{inner}</div>;
  }
  return (
    <div
      className={`h-full rounded-2xl border border-base-600 bg-base-800 p-6 ${
        hover
          ? 'transition-all duration-200 hover:-translate-y-1 hover:border-accent/50 hover:shadow-glow'
          : ''
      } ${className}`}
    >
      {children}
    </div>
  );
}
