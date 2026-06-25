// palette.ts — the shared phosphor token palette for the Remotion compositions.
//
// Single source of truth for the green-phosphor colours + mono stack used across
// Intro / Reveal / Recap, mirroring the CSS custom properties in src/style.css
// (the DOM side reads those vars; WebGL/Remotion can't, so this is their mirror).
// Kept import-free so it stays inside the lazy Remotion chunk without dragging
// anything onto the ⌘⌥⌃M summon hot path. Harness identity is deliberately NOT
// here — that lives in ../harnessTheme.ts (the single harness source) and is
// always paired colour + glyph + label.

/** Backdrop near-black (style.css `--bg`). */
export const BG = '#020403';
/** Phosphor green — primary text (style.css `--green`). */
export const GREEN = '#76ff9d';
/** Acid highlight — titles / sigil (style.css `--acid`). */
export const ACID = '#b8ff6b';
/** Verdict amber — the "confirmed / critical" accent (style.css `--amber`, war-room CORE_CONFIRM halo). */
export const AMBER = '#ff5a37';
/** Dim cage wire / rule (style.css `--dim`). */
export const DIM = '#1b6f3a';
/** Monospace stack — matches the `@font-face WardenMono` in style.css. */
export const MONO = 'WardenMono, Menlo, Consolas, monospace';
