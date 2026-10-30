import type { ReactNode } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { docsNav } from '../data/site';

interface DocsLayoutProps {
  title: string;
  description?: string;
  children: ReactNode;
}

// Shared two-column layout for the user-manual pages: a sidebar of doc links
// plus the article content.
export default function DocsLayout({ title, description, children }: DocsLayoutProps) {
  const { pathname } = useLocation();

  return (
    <div className="container-page py-10 lg:py-14">
      <div className="grid gap-10 lg:grid-cols-[220px_1fr]">
        {/* Sidebar */}
        <aside className="lg:sticky lg:top-24 lg:self-start">
          <p className="mb-3 text-xs font-semibold uppercase tracking-widest text-slate-500">
            User Manual
          </p>
          <nav className="flex flex-col gap-1" aria-label="Documentation">
            {docsNav.map((item) => {
              const active = pathname === item.href;
              return (
                <Link
                  key={item.href}
                  to={item.href}
                  aria-current={active ? 'page' : undefined}
                  className={`rounded-md px-3 py-2 text-sm transition-colors ${
                    active
                      ? 'bg-base-700 font-medium text-white'
                      : 'text-slate-400 hover:bg-base-800 hover:text-white'
                  }`}
                >
                  {item.label}
                </Link>
              );
            })}
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
