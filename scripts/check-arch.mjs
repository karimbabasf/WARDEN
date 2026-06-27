#!/usr/bin/env node
// Architecture guardrail (REFACTOR.md D1.4): enforce FSD-lite downward-only imports
// inside web/viz. Dependencies may point DOWN only:  app -> views -> modules -> shared.
// Modules may NOT import sibling modules. `dev/` and files at the web/viz root are exempt.
// No dependencies — run with `pnpm check:arch`. Exits non-zero on any violation.
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = fileURLToPath(new URL('../web/viz', import.meta.url));

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (/\.(ts|tsx)$/.test(name)) out.push(p);
  }
  return out;
}

// Classify a path segment list into { layer, module? }.
function classify(segments) {
  const [a, b] = segments;
  if (a === 'modules') return { layer: 'modules', module: b };
  if (a === 'app' || a === 'views' || a === 'shared' || a === 'dev') return { layer: a };
  return { layer: 'root' }; // files directly under web/viz (e.g. windowChrome.test.ts)
}

// May a file in `from` import target layer `tLayer` (module `tModule`)?
function allowed(from, tLayer, tModule) {
  if (from.layer === 'app') return true;                       // top layer: anything
  if (from.layer === 'views') return tLayer !== 'app';         // not upward into app
  if (from.layer === 'modules') {
    if (tLayer === 'shared') return true;                      // down into shared
    if (tLayer === 'modules') return tModule === from.module;  // self only, no siblings
    return false;                                              // views/app are upward
  }
  if (from.layer === 'shared') return tLayer === 'shared';     // shared depends on nothing above
  return true;                                                 // root: not enforced
}

const IMPORT_RE = /(?:from|import\()\s*['"]@\/viz\/([^'"]+)['"]/g;
const violations = [];

for (const file of walk(ROOT)) {
  const rel = file.slice(ROOT.length + 1);
  const from = classify(rel.split('/'));
  if (from.layer === 'dev' || from.layer === 'root') continue; // exempt
  const src = readFileSync(file, 'utf8');
  for (const m of src.matchAll(IMPORT_RE)) {
    const tSeg = m[1].split('/');
    const target = classify(tSeg);
    if (!allowed(from, target.layer, target.module)) {
      violations.push(`  ${rel}\n      imports @/viz/${m[1]}  (${from.layer}${from.module ? '/' + from.module : ''} -> ${target.layer}${target.module ? '/' + target.module : ''})`);
    }
  }
}

if (violations.length) {
  console.error(`\nArchitecture check FAILED — ${violations.length} import-direction violation(s):\n`);
  console.error(violations.join('\n'));
  console.error('\nRule: app -> views -> modules -> shared (down only); no sibling-module imports.\n');
  process.exit(1);
}
console.log('Architecture check OK — web/viz imports are downward-only, no cross-module coupling.');
