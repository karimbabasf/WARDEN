// harnessColors.test.ts — single source of truth for harness colour/glyph/label.
import { describe, expect, it } from 'vitest';
import { HARNESS_COLORS, KNOWN_HARNESS_IDS, harnessColor } from './harnessColors';

describe('HARNESS_COLORS constant', () => {
  it('claude_code has the canonical tangy-tangerine hue', () => {
    expect(HARNESS_COLORS.claude_code.hue).toBe('#ff8636');
  });

  it('codex has the canonical cyan-ice hue', () => {
    expect(HARNESS_COLORS.codex.hue).toBe('#4fc9ff');
  });

  it('unknown has the canonical slate hue', () => {
    expect(HARNESS_COLORS.unknown.hue).toBe('#8fa0b8');
  });

  it('each entry carries the canonical glyph', () => {
    expect(HARNESS_COLORS.claude_code.glyph).toBe('◆');
    expect(HARNESS_COLORS.codex.glyph).toBe('▣');
    expect(HARNESS_COLORS.unknown.glyph).toBe('◇');
  });

  it('each entry carries the canonical label', () => {
    expect(HARNESS_COLORS.claude_code.label).toBe('Claude');
    expect(HARNESS_COLORS.codex.label).toBe('Codex');
    expect(HARNESS_COLORS.unknown.label).toBe('Unknown');
  });

  it('ids are self-consistent', () => {
    expect(HARNESS_COLORS.claude_code.id).toBe('claude_code');
    expect(HARNESS_COLORS.codex.id).toBe('codex');
    expect(HARNESS_COLORS.unknown.id).toBe('unknown');
  });
});

describe('harnessColor()', () => {
  it('resolves claude_code to the tangerine hue', () => {
    expect(harnessColor('claude_code').hue).toBe('#ff8636');
  });

  it('resolves codex glyph', () => {
    expect(harnessColor('codex').glyph).toBe('▣');
  });

  it('unknown fallback for an unrecognised harness id', () => {
    expect(harnessColor('weird').id).toBe('unknown');
  });

  it('null input falls back to unknown', () => {
    expect(harnessColor(null).label).toBe('Unknown');
  });

  it('undefined input falls back to unknown', () => {
    expect(harnessColor(undefined).id).toBe('unknown');
  });

  it('empty string falls back to unknown', () => {
    expect(harnessColor('').id).toBe('unknown');
  });

  it('is case-insensitive (CLAUDE_CODE → claude_code)', () => {
    expect(harnessColor('CLAUDE_CODE').hue).toBe('#ff8636');
  });

  it('unknown fallback has the neutral slate colour (honest-viz)', () => {
    expect(harnessColor('gemini').hue).toBe('#8fa0b8');
  });

  it('unknown fallback colour is distinct from both brand hues', () => {
    const u = harnessColor('something_random');
    expect(u.hue).not.toBe('#ff8636');
    expect(u.hue).not.toBe('#4fc9ff');
  });
});

// ---------------------------------------------------------------------------
// Palette invariants. These guard the two properties the table has to keep as it
// grows: every harness has an identity, and no two are confusable on a dark field.
// ---------------------------------------------------------------------------

/** Hue angle in degrees, 0 to 360, from a #rrggbb string. */
function hueDeg(hex: string): number {
  const n = parseInt(hex.slice(1), 16);
  const r = ((n >> 16) & 255) / 255;
  const g = ((n >> 8) & 255) / 255;
  const b = (n & 255) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  if (d === 0) return 0;
  let h: number;
  if (max === r) h = ((g - b) / d) % 6;
  else if (max === g) h = (b - r) / d + 2;
  else h = (r - g) / d + 4;
  h *= 60;
  return h < 0 ? h + 360 : h;
}

/** Shortest distance between two hue angles, so 350 and 10 are 20 apart. */
function hueGap(a: number, b: number): number {
  const d = Math.abs(a - b) % 360;
  return d > 180 ? 360 - d : d;
}

describe('palette invariants', () => {
  it('gives every known harness its own entry', () => {
    for (const id of KNOWN_HARNESS_IDS) {
      const c = HARNESS_COLORS[id];
      expect(c, `${id} must have a colour entry`).toBeDefined();
      expect(c.hue).toMatch(/^#[0-9a-f]{6}$/);
      expect(c.label.length).toBeGreaterThan(0);
      expect(c.glyph.length).toBeGreaterThan(0);
    }
  });

  it('never repeats a hue, a glyph or a label', () => {
    const all = [...KNOWN_HARNESS_IDS, 'unknown' as const];
    for (const key of ['hue', 'glyph', 'label'] as const) {
      const seen = all.map((id) => HARNESS_COLORS[id][key]);
      expect(new Set(seen).size, `duplicate ${key}`).toBe(all.length);
    }
  });

  // The safety-critical one. `--alert` #ff2740 means an agent is STOPPED ON THE
  // OPERATOR. A harness identity sitting next to it on the wheel would train the
  // eye to discount the alarm, so every brand hue is held well clear of it.
  it('holds every harness hue away from the AWAITING alert red', () => {
    const alert = hueDeg('#ff2740');
    for (const id of KNOWN_HARNESS_IDS) {
      const gap = hueGap(hueDeg(HARNESS_COLORS[id].hue), alert);
      expect(gap, `${id} is too close to the alert red`).toBeGreaterThan(25);
    }
  });

  // Colour is never the only signal (every entry carries a glyph), so hues do not
  // have to be maximally far apart. They do have to be TELLABLE APART while dim,
  // which is what a globe at rest looks like.
  it('keeps distinct harnesses apart on the wheel', () => {
    // Claude and OpenClaw are the one deliberate near-pair: OpenClaw is a Claude
    // Code fork and the palette says so. Everything else clears a wider bar.
    const RELATED = new Set(['claude_code|openclaw']);
    for (const a of KNOWN_HARNESS_IDS) {
      for (const b of KNOWN_HARNESS_IDS) {
        if (a >= b) continue;
        const gap = hueGap(hueDeg(HARNESS_COLORS[a].hue), hueDeg(HARNESS_COLORS[b].hue));
        const floor = RELATED.has(`${a}|${b}`) || RELATED.has(`${b}|${a}`) ? 10 : 24;
        expect(gap, `${a} and ${b} are too close`).toBeGreaterThanOrEqual(floor);
      }
    }
  });

  it('keeps green out of the palette', () => {
    for (const id of KNOWN_HARNESS_IDS) {
      const h = hueDeg(HARNESS_COLORS[id].hue);
      expect(h < 90 || h > 160, `${id} is a green`).toBe(true);
    }
  });
});
