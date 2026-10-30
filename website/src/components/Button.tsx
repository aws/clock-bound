import { Link } from 'react-router-dom';
import type { ReactNode } from 'react';

type Variant = 'primary' | 'secondary' | 'ghost';
type Size = 'md' | 'lg';

interface ButtonProps {
  children: ReactNode;
  href?: string;
  variant?: Variant;
  size?: Size;
  external?: boolean;
  className?: string;
  ariaLabel?: string;
}

const base =
  'inline-flex items-center justify-center gap-2 rounded-lg font-medium transition-all duration-200 focus-visible:outline-none';

const variants: Record<Variant, string> = {
  primary:
    'bg-accent text-white shadow-glow hover:bg-accent-hover hover:-translate-y-0.5',
  secondary:
    'border border-slate-600 bg-base-800/60 text-slate-100 hover:border-accent hover:bg-base-700',
  ghost: 'text-slate-300 hover:text-white hover:bg-base-700',
};

const sizes: Record<Size, string> = {
  md: 'px-4 py-2 text-sm',
  lg: 'px-6 py-3 text-base',
};

export default function Button({
  children,
  href,
  variant = 'primary',
  size = 'md',
  external = false,
  className = '',
  ariaLabel,
}: ButtonProps) {
  const cls = `${base} ${variants[variant]} ${sizes[size]} ${className}`;

  if (href && external) {
    return (
      <a
        href={href}
        target="_blank"
        rel="noopener noreferrer"
        className={cls}
        aria-label={ariaLabel}
      >
        {children}
      </a>
    );
  }
  if (href) {
    return (
      <Link to={href} className={cls} aria-label={ariaLabel}>
        {children}
      </Link>
    );
  }
  return (
    <button type="button" className={cls} aria-label={ariaLabel}>
      {children}
    </button>
  );
}
