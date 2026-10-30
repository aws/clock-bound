import type { ReactNode } from 'react';
import HeadingAnchor from './HeadingAnchor';

interface SectionHeadingProps {
  eyebrow?: string;
  title: ReactNode;
  description?: ReactNode;
  centered?: boolean;
  /**
   * Optional section id. When set, the heading becomes a link target
   * (`#<id>`) and shows a "link to this section" icon after its title. Treat
   * it as stable once shared.
   */
  id?: string;
}

export default function SectionHeading({
  eyebrow,
  title,
  description,
  centered = true,
  id,
}: SectionHeadingProps) {
  return (
    <div className={`${centered ? 'mx-auto max-w-2xl text-center' : 'max-w-2xl'}`}>
      {eyebrow && (
        <p className="mb-3 text-sm font-semibold uppercase tracking-widest text-accent-hover">
          {eyebrow}
        </p>
      )}
      <h2
        id={id}
        className={`text-balance text-3xl font-bold tracking-tight text-white sm:text-4xl${
          id ? ' anchor-heading' : ''
        }`}
      >
        {title}
        {id && (
          <>
            {/* Non-breaking space keeps the icon with the title's last word. */}
            {'\u00A0'}
            <HeadingAnchor id={id} />
          </>
        )}
      </h2>
      {description && (
        <p className="mt-4 text-lg leading-relaxed text-slate-400">{description}</p>
      )}
    </div>
  );
}
