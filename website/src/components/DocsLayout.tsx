import type { ReactNode } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { docsNav } from '../data/site';

interface DocsLayoutProps {
  title: string;
  description?: string;
  children: ReactNode;
}

// Shared two-column layout for the docs pages: a sidebar of grouped doc links
// plus the article content.
export default function DocsLayout({ title, description, children }: DocsLayoutProps) {
  const { pathname } = useLocation();

  return (
    <div className="container-page py-10 lg:py-14">
      <div className="grid gap-10 lg:grid-cols-[220px_1fr]">
        {/* Sidebar */}
        <aside className="lg:sticky lg:top-24 lg:self-start">
          <nav className="flex flex-col gap-6" aria-label="Documentation">
            {docsNav.map((group) => (
              <div key={group.heading}>
                <p className="mb-3 text-xs font-semibold uppercase tracking-widest text-slate-500">
                  {group.heading}
                </p>
                <ul className="flex flex-col gap-1">
                  {group.items.map((item) => {
                    // Trailing slashes can appear on pre-rendered pages served
                    // as directories; compare without them.
                    const active = pathname.replace(/\/+$/, '') === item.href;
                    return (
                      <li key={item.href}>
                        <Link
                          to={item.href}
                          aria-current={active ? 'page' : undefined}
                          className={`block rounded-md py-2 pr-3 text-sm transition-colors ${
                            item.nested ? 'pl-7' : 'pl-3'
                          } ${
                            active
                              ? 'bg-base-700 font-medium text-white'
                              : 'text-slate-400 hover:bg-base-800 hover:text-white'
                          }`}
                        >
                          {item.label}
                        </Link>
                      </li>
                    );
                  })}
                </ul>
              </div>
            ))}
          </nav>
        </aside>

        {/* Content */}
        <article className="min-w-0">
          <header className="mb-8 border-b border-base-700 pb-6">
            <h1 className="text-3xl font-bold tracking-tight text-white sm:text-4xl">
              {title}
            </h1>
            {description && (
              <p className="mt-3 text-lg text-slate-400">{description}</p>
            )}
          </header>
          <div className="doc-prose">{children}</div>
        </article>
      </div>
    </div>
  );
}
