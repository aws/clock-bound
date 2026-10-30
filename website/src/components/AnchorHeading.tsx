import type { ReactNode } from 'react';
import HeadingAnchor from './HeadingAnchor';

interface AnchorHeadingProps {
  level: 2 | 3;
  /** Section id. Becomes the URL fragment, so treat it as stable once shared. */
  id: string;
  children: ReactNode;
}

// A docs heading with a "link to this section" icon after its text (see
// HeadingAnchor). The non-breaking space keeps the icon on the same line as
// the last word, so a wrapping heading never leaves the icon alone on a line.
export default function AnchorHeading({ level, id, children }: AnchorHeadingProps) {
  const Tag = level === 2 ? 'h2' : 'h3';
  return (
    <Tag id={id} className="anchor-heading">
      {children}
      {'\u00A0'}
      <HeadingAnchor id={id} />
    </Tag>
  );
}
