# ClockBound Website

The public-facing product landing site for **ClockBound v3** — a static
React + Vite + Tailwind CSS site. It builds to plain HTML/CSS/JS with **no
backend**, so it can be hosted on any plain HTTP server (Amazon S3 +
CloudFront, GitHub Pages, nginx, etc.).

> **Status: placeholder.** The site currently renders a single
> "ClockBound v3 website coming soon …" page. The full build system,
> configuration, and GitHub Pages deployment workflow are already in place, so
> the real content can be dropped in later without reworking the tooling.

## Develop

```sh
cd website
npm install
npm run dev          # local dev server at http://localhost:5173
```

## Build (static output)

```sh
npm run build        # pre-renders every route to dist/ as real .html files
npm run preview      # serve the production build locally to verify
```

The `dist/` directory is the deployable artifact. Every route is emitted as a
standalone `.html` file via `vite-react-ssg`, so deep links work on a dumb file
server with no rewrite rules required.

### Hosting path

The base path is env-driven via `VITE_BASE` (see `vite.config.ts`):

- Local dev / root hosting (custom domain, user/org Pages site): `/`
- GitHub Pages **project** site at `https://<org>.github.io/clock-bound/`:
  the CI workflow sets `VITE_BASE` from `actions/configure-pages`, so assets
  resolve under the `/clock-bound/` subpath automatically.

## Structure

```
src/
  pages/        Route-level pages (LandingPage placeholder, NotFound)
  data/         Build-time stats (github-stats.json)
  routes.tsx    Route table consumed by vite-react-ssg
  main.tsx      SSG entry point
  index.css     Tailwind layers + base theme
scripts/
  copy-assets.mjs   Copies canonical repo assets into public/ (currently no-op)
  fetch-stats.mjs   Build-time GitHub/crates.io stats fetch (degrades to null)
```

## Build-time GitHub / crates.io stats

`scripts/fetch-stats.mjs` runs automatically before `npm run build` and
`npm run dev`. It queries the GitHub REST API (`aws/clock-bound`) and crates.io
(`clock-bound`), writing the result to `src/data/github-stats.json`.

- Any field that can't be fetched is written as `null` (the placeholder page
  does not currently display these, but the plumbing is kept intact for the
  real site). An offline, rate-limited, or pre-public build simply writes
  `null`s and still succeeds.
- A committed all-`null` seed of `github-stats.json` lives in the repo so fresh
  checkouts type-check and build before the script has run.
- Set `GITHUB_TOKEN` in the build/CI environment to raise the GitHub rate limit
  (and to read the repo before it is public).

## Deployment

`.github/workflows/website_build_and_deploy.yml` builds this directory and
deploys `dist/` to GitHub Pages via GitHub Actions on pushes that touch
`website/**` (or the workflow file itself).
