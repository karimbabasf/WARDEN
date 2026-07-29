#!/usr/bin/env node
// site/build.mjs
//
// Vercel's buildCommand for this project. landing/ is the single source of
// truth for the marketing page; this script copies the servable parts of it
// into site/public (generated, gitignored -- see site/.gitignore) and fills
// in any __PUBLIC_X__ placeholder the page uses with the matching PUBLIC_*
// env var. Dependency-free: only Node built-ins, no npm install required to
// run it.
//
// Placeholder convention: a token written as __PUBLIC_FOO__ anywhere in a
// copied text file is replaced with process.env.PUBLIC_FOO. If the token is
// present but the env var is not set, the build fails loudly rather than
// shipping a page with a literal placeholder or a silently-empty key baked
// into a payments page. If no such token exists in the page (the current
// state of landing/index.html), this step is a no-op.

import { cpSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const SITE_DIR = path.dirname(fileURLToPath(import.meta.url));
const LANDING_DIR = path.join(SITE_DIR, '..', 'landing');
const PUBLIC_DIR = path.join(SITE_DIR, 'public');

// Build-time-only or verification-artifact entries in landing/ that must not
// ship: the esbuild source and script that produce vendor/w3.js (the built
// bundle itself is what ships), and look.mjs audit screenshots.
const EXCLUDE = new Set(['.design', 'src', 'build-vendor.mjs', '.DS_Store', 'node_modules']);

// Extensions worth scanning for __PUBLIC_X__ placeholders. Fonts and images
// are copied byte-for-byte and never opened as text.
const TEXT_EXTS = new Set(['.html', '.htm', '.js', '.mjs', '.css', '.json', '.svg', '.txt']);

function shouldCopy(src) {
  const rel = path.relative(LANDING_DIR, src);
  if (rel === '') return true;
  const top = rel.split(path.sep)[0];
  return !EXCLUDE.has(top);
}

function walk(dir, out = []) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full, out);
    else out.push(full);
  }
  return out;
}

function injectPublicEnv(files) {
  const tokenPattern = /__PUBLIC_[A-Z0-9_]+__/g;
  const seen = new Map(); // varName -> replaced count
  const missing = new Set();

  for (const file of files) {
    if (!TEXT_EXTS.has(path.extname(file))) continue;
    const original = readFileSync(file, 'utf8');
    if (!tokenPattern.test(original)) continue;
    tokenPattern.lastIndex = 0;

    const updated = original.replace(tokenPattern, (token) => {
      const varName = token.slice(2, -2); // strip leading/trailing __
      const value = process.env[varName];
      if (value === undefined) {
        missing.add(varName);
        return token;
      }
      seen.set(varName, (seen.get(varName) ?? 0) + 1);
      return value;
    });

    if (updated !== original) writeFileSync(file, updated);
  }

  return { seen, missing };
}

function main() {
  console.log(`Copying ${path.relative(SITE_DIR, LANDING_DIR)} -> ${path.relative(SITE_DIR, PUBLIC_DIR)}`);
  rmSync(PUBLIC_DIR, { recursive: true, force: true });
  cpSync(LANDING_DIR, PUBLIC_DIR, { recursive: true, filter: shouldCopy });

  const files = walk(PUBLIC_DIR);
  console.log(`Copied ${files.length} files.`);

  const { seen, missing } = injectPublicEnv(files);
  for (const [varName, count] of seen) {
    console.log(`Injected ${varName} (${count} occurrence${count === 1 ? '' : 's'}).`);
  }
  if (seen.size === 0 && missing.size === 0) {
    console.log('No __PUBLIC_*__ placeholders found in the copied page (nothing to inject).');
  }

  if (missing.size > 0) {
    for (const varName of missing) {
      console.error(`ERROR: the page uses __${varName}__ but process.env.${varName} is not set.`);
    }
    throw new Error(`missing ${missing.size} required PUBLIC_* env var(s): ${[...missing].join(', ')}`);
  }

  const stat = statSync(path.join(PUBLIC_DIR, 'index.html'));
  console.log(`Build ok. index.html: ${(stat.size / 1024).toFixed(1)} KB`);
}

try {
  main();
} catch (err) {
  console.error(`\nERROR: ${err.message}`);
  process.exit(1);
}
