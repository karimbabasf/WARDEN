#!/usr/bin/env node
// Packs an LDraw model and every library file it references into one MPD, so the
// Brickyard view can load a set with zero network and zero library lookups at runtime.
//
//   node scripts/brickyard/pack.mjs <ldraw-library-dir> <out-dir> <model.mpd|ldr>...
//
// Embedded FILE names follow LDrawLoader's own normalization (`s/` -> `parts/s/`,
// `48/` -> `p/48/`), so the loader finds every subfile in its cache and never fetches.
// The colour table is not packed: the view preloads one shared LDConfig.ldr instead.

import fs from 'node:fs';
import path from 'node:path';

const [, , libDir, outDir, ...models] = process.argv;
if (!libDir || !outDir || models.length === 0) {
  console.error('usage: pack.mjs <ldraw-library-dir> <out-dir> <model>...');
  process.exit(1);
}

function loaderName(ref) {
  let name = ref.trim().replace(/\\/g, '/');
  if (name.startsWith('s/')) name = `parts/${name}`;
  else if (name.startsWith('48/')) name = `p/${name}`;
  return name;
}

function locate(name) {
  const lower = name.toLowerCase();
  const candidates = [
    path.join(libDir, 'parts', lower),
    path.join(libDir, 'p', lower),
    path.join(libDir, lower),
    path.join(libDir, 'models', lower),
  ];
  return candidates.find((p) => fs.existsSync(p)) ?? null;
}

function refsOf(text) {
  const out = [];
  for (const line of text.split('\n')) {
    const t = line.trim();
    if (!t.startsWith('1 ')) continue;
    const tokens = t.split(/\s+/);
    if (tokens.length < 15) continue;
    out.push(loaderName(tokens.slice(14).join(' ')));
  }
  return out;
}

function embeddedNames(text) {
  const names = new Set();
  for (const line of text.split('\n')) {
    const m = line.match(/^0 FILE (.+)$/i);
    if (m) names.add(loaderName(m[1]).toLowerCase());
  }
  return names;
}

fs.mkdirSync(outDir, { recursive: true });
for (const model of models) {
  // The loader stores an embedded file under its FILE line verbatim but looks a
  // reference up trimmed and normalized, so a trailing space or an `s\` prefix on a
  // FILE line makes a model's own submodel unfindable. Rewrite them to match.
  const main = fs
    .readFileSync(model, 'utf8')
    .replace(/\r\n?/g, '\n')
    .replace(/^0 FILE (.+)$/gim, (_, name) => `0 FILE ${loaderName(name)}`);
  const known = embeddedNames(main);
  const packed = new Map();
  const missing = new Set();
  const queue = refsOf(main);
  while (queue.length) {
    const name = queue.shift();
    const key = name.toLowerCase();
    if (known.has(key) || packed.has(key) || missing.has(key)) continue;
    const file = locate(name);
    if (!file) {
      missing.add(key);
      continue;
    }
    const text = fs.readFileSync(file, 'utf8').replace(/\r\n?/g, '\n');
    packed.set(key, { name, text });
    queue.push(...refsOf(text));
  }
  // A model with no FILE header is a plain LDR: give it one so the embedded files
  // that follow are read as embedded files, not as more lines of the main model.
  const head = /^0 FILE /im.test(main.split('\n').find((l) => l.trim()) ?? '')
    ? main
    : `0 FILE ${path.basename(model)}\n${main}`;
  let out = head.trimEnd() + '\n';
  for (const { name, text } of packed.values()) out += `0 FILE ${name}\n${text.trimEnd()}\n`;
  const dest = path.join(outDir, path.basename(model).replace(/\.(ldr|mpd)$/i, '.mpd'));
  fs.writeFileSync(dest, out);
  console.log(
    `${path.basename(dest)}: ${packed.size} library files, ${(out.length / 1024).toFixed(0)} KB` +
      (missing.size ? `, MISSING ${[...missing].join(' ')}` : ''),
  );
}
