// Web mirror of the Rust `harness_theme` (src-tauri/src/ingest/mod.rs). This is
// the SINGLE source of harness identity on the web side — colours, glyphs and
// labels live here and nowhere else so the legend, cage rims and any future HUD
// all agree. Always pair the colour with the glyph + label (color-blind a11y):
// colour alone is never a signal in WARDEN.
//
// Harness colour/glyph/label literals live in `harnessColors.ts` (one source of
// truth for both Habits and Radar). This file re-exposes them as the HarnessTheme
// shape that existing consumers expect, without duplicating any values.

import { harnessColor } from './harnessColors';

export type HarnessTheme = {
  /** Human label rendered in the legend, e.g. "Claude". */
  label: string;
  /** Secondary-accent hex (cage rim + legend swatch), NOT the verdict colour. */
  color: string;
  /** Glyph paired with the colour so the harness is legible without colour. */
  glyph: string;
};

// Keys are snake_case harness ids exactly as the Rust side emits them
// ("claude_code", "codex"). Anything else falls through to `NEUTRAL`.
// Values are sourced from harnessColors — no literals duplicated here.
const _cl = harnessColor('claude_code');
const _cx = harnessColor('codex');
export const HARNESS = {
  claude_code: { label: _cl.label, color: _cl.hue, glyph: _cl.glyph },
  codex:       { label: _cx.label, color: _cx.hue, glyph: _cx.glyph },
} as const satisfies Record<string, HarnessTheme>;

// Schema drift / off-Fugu / unknown harnesses degrade to a quiet slate chip
// rather than borrowing another harness's identity.
const _un = harnessColor('unknown');
export const NEUTRAL: HarnessTheme = { label: _un.label, color: _un.hue, glyph: _un.glyph };

export type HarnessId = keyof typeof HARNESS;

/** Resolve a (possibly unknown) snake_case harness id to its theme. */
export function harnessTheme(h: string): HarnessTheme {
  return (HARNESS as Record<string, HarnessTheme>)[h] ?? NEUTRAL;
}

// Severity ramp — a vivid heat scale that glows: a calm, clear sky-blue at the
// low end climbing through electric amber → orange → crimson. Saturated on
// purpose so danger reads instantly and the scene stays alive, not dull.
export function severityColor(severity: number): string {
  const s = Number.isFinite(severity) ? Math.round(severity) : 0;
  if (s <= 2) return '#54c6ff'; // luminous sky blue — calm / clear
  if (s === 3) return '#ffd23e'; // electric amber
  if (s === 4) return '#ff9332'; // vivid orange
  return '#ff3d52'; // hot crimson
}
