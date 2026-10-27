// Copies canonical repo assets into website/public/ at build time so the site
// stays self-contained and portable without committing duplicate binaries.
// The canonical source of truth remains in the repo's docs/assets/ directory;
// the copies under public/img/ are gitignored build artifacts.
//
// There are currently no assets to copy. This script is kept as a no-op
// scaffold: add [source, destination] pairs to `assets` to restore the
// behaviour for any future binary that must be embedded.
import { copyFileSync, mkdirSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const websiteRoot = resolve(here, '..');
const repoRoot = resolve(websiteRoot, '..');

// [source (relative to repo root), destination (relative to website root)]
const assets = [];

if (assets.length > 0) {
  mkdirSync(resolve(websiteRoot, 'public/img'), { recursive: true });
  for (const [src, dest] of assets) {
    const from = resolve(repoRoot, src);
    const to = resolve(websiteRoot, dest);
    copyFileSync(from, to);
    console.log(`[copy-assets] ${src} -> ${dest}`);
  }
} else {
  console.log('[copy-assets] no assets to copy');
}
