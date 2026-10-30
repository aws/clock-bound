# ClockBound Website

The public-facing product landing site for **ClockBound v3** — a static
React + Vite + Tailwind CSS site. It builds to plain HTML/CSS/JS with **no
backend**, so it can be hosted on any plain HTTP server (Amazon S3 +
CloudFront, GitHub Pages, nginx, etc.).

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

The `dist/` directory is the deployable artifact. Every route
(`/`, `/getting-started`, `/docs/*`) is emitted as a standalone `.html`
file via `vite-react-ssg`, so deep links work on a dumb file server with no
rewrite rules required.

### Hosting path

The site is built with `base: '/'` in `vite.config.ts`, meaning it is served
from the **domain root** (e.g. `https://clockbound.example.com/`). This covers
S3 + CloudFront, nginx, and GitHub Pages user/org sites.

To host under a **subpath** instead (e.g. a GitHub project page at
`https://org.github.io/clock-bound/`), change `base` to the matching prefix
(`'/clock-bound/'`) and rebuild. A relative `base: './'` does **not** work here
because nested `/docs/*` pages would resolve assets from the wrong directory.

## Structure

```
src/
  components/   Reusable design-system components (NavBar, Footer, Button, ...)
  pages/        Route-level pages (Landing, GettingStarted, docs/*)
  data/         Content data (features, FAQ, stats, nav links)
  routes.tsx    Route table consumed by vite-react-ssg
  main.tsx      SSG entry point
  index.css     Tailwind layers + base theme
```

## Build-time GitHub / crates.io stats

The stats bar (Stars, Forks, Downloads, Contributors) is populated at build
time by `scripts/fetch-stats.mjs`, which runs automatically before
`npm run build` and `npm run dev` (you can also run it directly with
`npm run fetch-stats`). It queries the GitHub REST API (`aws/clock-bound`) for
stars, forks, and the contributor count, and crates.io (`clock-bound`) for
downloads, writing the result to `src/data/github-stats.json`.

- **Failed fetches keep the previous value.** If a field can't be fetched
  (offline, rate-limited, repo not yet public, etc.), the script keeps the
  value already in `github-stats.json` and logs
  `[fetch-stats] <field>: fetch failed, keeping previous value N`. Each failed
  request is also logged with its HTTP status (e.g. `-> HTTP 403 (skipping)`).
  A field is written as `null` (shown in the UI as `—`) only when there is no
  previous value to fall back to.
- **`fetchedAt`** is updated only when at least one field was freshly fetched,
  so it reflects when the data was last real. A number shown on the site may
  therefore be stale; check `fetchedAt` if one seems old.
- `github-stats.json` is committed so fresh checkouts type-check and build
  before the script has run. Builds overwrite it locally; it is not
  auto-committed.
- For always-fresh numbers, run the build on a schedule (e.g. a nightly CI
  job that rebuilds and redeploys).

### GitHub rate limiting and `GITHUB_TOKEN`

Without a token, the GitHub API allows only **60 requests per hour per IP
address**. Each build or dev-server start makes 2 GitHub requests, and the
limit is shared with anything else behind the same outbound IP (common on
corporate networks and dev desktops), so it is easy to exhaust. When that
happens GitHub returns `HTTP 403` and the GitHub-sourced stats keep their
previous values instead of updating.

Setting `GITHUB_TOKEN` raises the limit to 5,000 requests per hour (and lets
the script read the repo before it is public). The easiest way to get a token
locally is the [GitHub CLI](https://cli.github.com/):

```sh
gh auth status || gh auth login      # log in if not already logged in
export GITHUB_TOKEN=$(gh auth token)
npm run build                        # or: npm run dev / npm run fetch-stats
```

In CI, set `GITHUB_TOKEN` in the build environment instead (GitHub Actions
provides one as `secrets.GITHUB_TOKEN`).

## Content provenance / TODOs

Copy is derived from the ClockBound repo (`README.md`, `docs/*.md`,
`CHANGELOG.md`). Items that need a human to verify or supply real data are
marked with `TODO:` comments in the source. Known open items:

- Two diagrams (point-vs-window, architecture) are rendered as inline SVG
  placeholders — replace with final designed assets when available.
