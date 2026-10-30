import { useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import { Clock, Github, Menu, X } from 'lucide-react';
import { navLinks, siteConfig } from '../data/site';

export default function NavBar() {
  const [open, setOpen] = useState(false);
  const location = useLocation();

  const isActive = (href: string) =>
    href === '/' ? location.pathname === '/' : location.pathname.startsWith(href);

  return (
    <header className="sticky top-0 z-50 border-b border-base-700/60 bg-base-900/80 backdrop-blur-md">
      <nav className="container-page flex h-16 items-center justify-between" aria-label="Primary">
        <Link to="/" className="flex items-center gap-2 font-semibold text-white">
          <Clock size={22} className="text-accent" />
          <span className="text-lg">{siteConfig.name}</span>
          <span className="rounded bg-base-700 px-1.5 py-0.5 font-mono text-[10px] text-slate-400">
            v3
          </span>
        </Link>

        {/* Desktop nav */}
        <div className="hidden items-center gap-1 md:flex">
          {navLinks.map((link) => (
            <Link
              key={link.href}
              to={link.href}
              className={`rounded-md px-3 py-2 text-sm font-medium transition-colors ${
                isActive(link.href)
                  ? 'text-white'
                  : 'text-slate-400 hover:text-white'
              }`}
            >
              {link.label}
            </Link>
          ))}
          <a
            href={siteConfig.githubUrl}
            target="_blank"
            rel="noopener noreferrer"
            className="ml-2 inline-flex items-center gap-2 rounded-lg border border-slate-600 bg-base-800/60 px-3 py-2 text-sm font-medium text-slate-100 transition-colors hover:border-accent hover:bg-base-700"
          >
            <Github size={16} /> GitHub
          </a>
        </div>

        {/* Mobile toggle */}
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          className="rounded-md p-2 text-slate-300 hover:bg-base-700 hover:text-white md:hidden"
          aria-label={open ? 'Close menu' : 'Open menu'}
          aria-expanded={open}
          aria-controls="mobile-menu"
        >
          {open ? <X size={22} /> : <Menu size={22} />}
        </button>
      </nav>

      {/* Mobile menu */}
      {open && (
        <div
          id="mobile-menu"
          className="border-t border-base-700/60 bg-base-900 md:hidden"
        >
          <div className="container-page flex flex-col gap-1 py-4">
            {navLinks.map((link) => (
              <Link
                key={link.href}
                to={link.href}
                onClick={() => setOpen(false)}
                className={`rounded-md px-3 py-2 text-base font-medium transition-colors ${
                  isActive(link.href)
                    ? 'bg-base-700 text-white'
                    : 'text-slate-300 hover:bg-base-700 hover:text-white'
                }`}
              >
                {link.label}
              </Link>
            ))}
            <a
              href={siteConfig.githubUrl}
              target="_blank"
              rel="noopener noreferrer"
              onClick={() => setOpen(false)}
              className="mt-1 inline-flex items-center gap-2 rounded-md px-3 py-2 text-base font-medium text-slate-300 hover:bg-base-700 hover:text-white"
            >
              <Github size={18} /> GitHub
            </a>
          </div>
        </div>
      )}
    </header>
  );
}
