// build-vendor.mjs: bundle three + postprocessing into one classic script.
//
// Run from the repo root: node landing/build-vendor.mjs
// esbuild is resolved out of the pnpm store because it is a transitive dep of
// vite and has no top-level bin link.
import { build } from '../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild/lib/main.js';
import { statSync } from 'node:fs';

const out = 'landing/vendor/w3.js';

await build({
  entryPoints: ['landing/src/vendor-entry.js'],
  bundle: true,
  format: 'iife',
  globalName: 'W3',
  minify: true,
  target: ['safari17', 'chrome120'],
  legalComments: 'none',
  outfile: out,
});

console.log(`${out}  ${(statSync(out).size / 1024).toFixed(1)}KB`);
