import { Link } from 'react-router-dom';
import { Clock, Github, Package } from 'lucide-react';
import { siteConfig } from '../data/site';

export default function Footer() {
  const year = new Date().getFullYear();
  return (
    <footer className="border-t border-base-700/60 bg-base-900">
      <div className="container-page py-12">
        <div className="grid gap-8 md:grid-cols-4">
          <div className="md:col-span-2">
            <Link to="/" className="flex items-center gap-2 font-semibold text-white">
              <Clock size={20} className="text-accent" />
              {siteConfig.name}
            </Link>
            <p className="mt-3 max-w-sm text-sm text-slate-400">
              Bounded, trustworthy time for distributed systems. Open source,
              dual-licensed under {siteConfig.license.replace(' OR ', ' / ')}.
            </p>
          </div>

          <div>
            <h3 className="mb-3 text-sm font-semibold text-white">Product</h3>
            <ul className="space-y-2 text-sm text-slate-400">
              <li>
                <Link to="/getting-started" className="hover:text-white">
                  Getting Started
                </Link>
              </li>
              <li>
                <Link to="/docs/concepts" className="hover:text-white">
                  Documentation
                </Link>
              </li>
              <li>
                <Link to="/docs/protocol" className="hover:text-white">
                  Protocol Reference
                </Link>
              </li>
            </ul>
          </div>

          <div>
            <h3 className="mb-3 text-sm font-semibold text-white">Community</h3>
            <ul className="space-y-2 text-sm text-slate-400">
              <li>
                <a
                  href={siteConfig.githubUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center gap-2 hover:text-white"
                >
                  <Github size={14} /> GitHub
                </a>
              </li>
              <li>
                <a
                  href={siteConfig.cratesUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="inline-flex items-center gap-2 hover:text-white"
                >
                  <Package size={14} /> crates.io
                </a>
              </li>
              <li>
                <a
                  href={siteConfig.contributingUrl}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="hover:text-white"
                >
                  Contributing
                </a>
              </li>
            </ul>
          </div>
        </div>

        <div className="mt-10 flex flex-col items-center justify-between gap-3 border-t border-base-700/60 pt-6 text-sm text-slate-500 sm:flex-row">
          <p>
            © {year} {siteConfig.copyrightHolder}. All rights reserved.
          </p>
          <p className="font-mono text-xs">
            {siteConfig.license} · v{siteConfig.version}
          </p>
        </div>
      </div>
    </footer>
  );
}
