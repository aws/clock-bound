import { Link as LinkIcon } from 'lucide-react';

// The "link to this section" icon placed after a heading's text. A plain
// fragment link: clicking it jumps to the section and puts `#<id>` in the
// address bar. Styled by `.heading-anchor` in index.css: shown when the
// enclosing `.anchor-heading` is hovered or the link has keyboard focus, and
// faintly visible on touch screens. The icon scales with the heading's font.
export default function HeadingAnchor({ id }: { id: string }) {
  return (
    <a
      href={`#${id}`}
      className="heading-anchor"
      aria-label="Link to this section"
      title="Link to this section"
    >
      <LinkIcon size="0.7em" aria-hidden="true" />
    </a>
  );
}
