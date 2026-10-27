import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Normalize a base path from the environment into a form Vite accepts.
// Vite requires the base to start AND end with "/". The CI workflow feeds this
// from actions/configure-pages' `base_path` output, which may arrive as
// "/clock-bound", "clock-bound", "/clock-bound/", or "/" (root). This coerces
// any of those to a valid value ("/" stays "/"), so a missing or odd-format
// value can never produce broken asset URLs.
function normalizeBase(b?: string): string {
  if (!b || b === '/') return '/';
  const trimmed = b.replace(/^\/+|\/+$/g, '');
  return trimmed === '' ? '/' : `/${trimmed}/`;
}

// Static site build. `vite-react-ssg` pre-renders every route defined in
// src/routes.tsx to a real .html file at build time, so the output in dist/
// is pure static HTML/CSS/JS that can be hosted on any plain HTTP server
// (S3 + CloudFront, GitHub Pages, nginx, etc.) with no application backend.
export default defineConfig({
  plugins: [react()],
  // Absolute base so asset URLs resolve the same way on every page regardless
  // of its directory depth.
  //
  // The base is env-driven so that one build config can work in all scenarios:
  //   - Local dev / root hosting (private *.pages.github.io, custom domain): '/'
  //   - GitHub Pages PROJECT site at https://<org>.github.io/<repo>/:
  //     VITE_BASE=/<repo>/ (the CI workflow sets this from configure-pages).
  // normalizeBase() tolerates missing or no-trailing-slash values.
  base: normalizeBase(process.env.VITE_BASE),
  // Dev server only. Bind to all interfaces so the site is reachable via the
  // dev desk's external hostname (not just loopback), and skip Vite's
  // Host-header allowlist so any dev-desk hostname works. Does not affect the
  // static build or the deployed site.
  server: {
    host: '0.0.0.0',
    port: 5173,
    allowedHosts: true,
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
});
