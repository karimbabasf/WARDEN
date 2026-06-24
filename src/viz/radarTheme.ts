// radarTheme.ts — RADAR's OWN palette + heat ramp. SEPARATE from Habits'
// `harnessTheme.HARNESS` (which stays the anti-pattern map). Per the locked
// decision log: Radar is Claude **orange**, Codex **violet**, and a globe's
// colour is its harness hue HEATED BY FILL — deep dim ember when the context
// window is near-empty, climbing through brighter, to a blazing white-hot core
// when it is near-full. Harness identity stays legible at every fill level, and
// colour is ALWAYS paired with a glyph + label (color-blind a11y).
//
// Pure module: no Three.js import, so it is trivially unit-testable and shared by
// the layout, the render and the detail panel without dragging in WebGL.
//
// Harness colour/glyph/label literals live in `harnessColors.ts` — no duplication.

import { harnessColor } from './harnessColors';

export type RadarTheme = {
  /** Human label rendered in cards/legend, e.g. "Claude". */
  label: string;
  /** Base harness hue (the ember/heat ramp is anchored on this). */
  color: string;
  /** Glyph paired with the colour so the harness is legible without colour. */
  glyph: string;
};

// Keys are snake_case harness ids exactly as the backend emits them.
// Values sourced from harnessColors — no literals duplicated here.
const _cl = harnessColor('claude_code');
const _cx = harnessColor('codex');
export const RADAR_PALETTE = {
  claude_code: { label: _cl.label, color: _cl.hue, glyph: _cl.glyph },
  codex:       { label: _cx.label, color: _cx.hue, glyph: _cx.glyph },
} as const satisfies Record<string, RadarTheme>;

// Unknown / schema-drift harness — a quiet slate globe, never borrowing another
// harness's identity (honest-viz). Distinct hue from both brand colours.
const _un = harnessColor('unknown');
export const RADAR_NEUTRAL: RadarTheme = { label: _un.label, color: _un.hue, glyph: _un.glyph };

export type RadarHarnessId = keyof typeof RADAR_PALETTE;

/** Resolve a (possibly unknown) snake_case harness id to its Radar theme. */
export function radarHarness(h: string): RadarTheme {
  return (RADAR_PALETTE as Record<string, RadarTheme>)[h] ?? RADAR_NEUTRAL;
}

// ── hex <-> rgb plumbing ───────────────────────────────────────────────────────
type Rgb = { r: number; g: number; b: number };

function parseHex(hex: string): Rgb | null {
  const m = /^#?([0-9a-fA-F]{6})$/.exec(hex.trim());
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return { r: (n >> 16) & 0xff, g: (n >> 8) & 0xff, b: n & 0xff };
}

function toHex({ r, g, b }: Rgb): string {
  const clampByte = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
  const h = (v: number) => clampByte(v).toString(16).padStart(2, '0');
  return `#${h(r)}${h(g)}${h(b)}`;
}

function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

function clamp01(v: number): number {
  if (!Number.isFinite(v)) return 0;
  return v < 0 ? 0 : v > 1 ? 1 : v;
}

/**
 * Heat a base harness hue by `fillPct` (0..1).
 *
 *   fill = 0   → a deep, dim ember of the hue (the window is nearly empty).
 *   fill rises → the hue brightens and warms.
 *   fill = 1   → a blazing white-hot core (all channels near max).
 *
 * Implemented as two stacked moves on the base colour:
 *   1) a brightness gain that scales the base up from a dim floor (EMBER_FLOOR)
 *      toward its full intensity as fill climbs — luminance rises monotonically;
 *   2) a whiteness lerp toward #fff that only takes over in the upper fill range
 *      (an eased curve), so mid fills keep the harness hue recognisable while
 *      near-full goes white-hot.
 *
 * A non-hex input is returned untouched (defensive — never throw in the render).
 */
export function heatColor(baseHex: string, fillPct: number): string {
  const base = parseHex(baseHex);
  if (!base) return baseHex;
  const f = clamp01(fillPct);

  // 1) brightness gain — even an empty globe is a visible dim ember, not black.
  const EMBER_FLOOR = 0.34; // base scaled to 34% at fill 0 …
  const gain = EMBER_FLOOR + (1 - EMBER_FLOOR) * f; // … up to 100% at fill 1
  const lit: Rgb = { r: base.r * gain, g: base.g * gain, b: base.b * gain };

  // 2) whiteness — eased so it engages mainly in the top of the range. f^2.2
  // keeps the hue legible at low/mid fill, then rushes to white-hot near full.
  const whiteness = Math.pow(f, 2.2) * 0.92;
  const out: Rgb = {
    r: lerp(lit.r, 255, whiteness),
    g: lerp(lit.g, 255, whiteness),
    b: lerp(lit.b, 255, whiteness),
  };

  return toHex(out);
}
