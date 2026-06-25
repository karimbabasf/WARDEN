import { describe, expect, it } from 'vitest';
import { RADAR_PALETTE, radarHarness, heatColor } from './radarTheme';

// Relative luminance (sRGB-ish, perceptual weights) — enough to assert that one
// colour is brighter than another without pulling in a colour library.
function luminance(hex: string): number {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) throw new Error(`not a hex colour: ${hex}`);
  const n = parseInt(m[1], 16);
  const r = (n >> 16) & 0xff;
  const g = (n >> 8) & 0xff;
  const b = n & 0xff;
  return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
}

function channels(hex: string): [number, number, number] {
  const n = parseInt(/^#?([0-9a-f]{6})$/i.exec(hex.trim())![1], 16);
  return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
}

describe('RADAR_PALETTE', () => {
  it('is vibrant + maximally distinct: Claude orange + Codex blue', () => {
    expect(RADAR_PALETTE.claude_code.color).toBe('#ff7a18');
    expect(RADAR_PALETTE.codex.color).toBe('#2e8bff');
  });

  it('always pairs colour with a glyph + label (color-blind a11y)', () => {
    expect(RADAR_PALETTE.claude_code.glyph.length).toBeGreaterThan(0);
    expect(RADAR_PALETTE.claude_code.label).toBe('Claude');
    expect(RADAR_PALETTE.codex.glyph.length).toBeGreaterThan(0);
    expect(RADAR_PALETTE.codex.label).toBe('Codex');
  });
});

describe('radarHarness', () => {
  it('resolves known harnesses', () => {
    expect(radarHarness('codex').color).toBe('#2e8bff');
    expect(radarHarness('claude_code').color).toBe('#ff7a18');
  });

  it('falls back to a neutral chip for unknown harnesses', () => {
    const n = radarHarness('gemini');
    expect(n.label).toBe('Unknown');
    expect(n.color).not.toBe('#ff7a18');
    expect(n.color).not.toBe('#2e8bff');
    expect(n.glyph.length).toBeGreaterThan(0);
  });
});

describe('heatColor — ember → white-hot ramp within the base hue', () => {
  it('brightens monotonically with fill', () => {
    const base = '#ff7a18';
    const lo = luminance(heatColor(base, 0));
    const mid = luminance(heatColor(base, 0.5));
    const hi = luminance(heatColor(base, 1));
    expect(mid).toBeGreaterThan(lo);
    expect(hi).toBeGreaterThan(mid);
  });

  it('empty reads as a deep dim ember (much darker than full)', () => {
    const base = '#ff7a18';
    expect(luminance(heatColor(base, 0))).toBeLessThan(luminance(base));
    expect(luminance(heatColor(base, 0))).toBeLessThan(luminance(heatColor(base, 1)) * 0.6);
  });

  it('near-full trends toward white-hot (all channels high)', () => {
    const [r, g, b] = channels(heatColor('#ff7a18', 1));
    expect(r).toBeGreaterThan(200);
    expect(g).toBeGreaterThan(200);
    expect(b).toBeGreaterThan(180);
  });

  it('preserves harness identity at mid fill (the hue is still recognisable)', () => {
    // At a middling fill the Claude orange must still read warm (R dominant),
    // and Codex violet must still read cool (B dominant) — heat never flips hue.
    const [cr, , cb] = channels(heatColor('#ff7a18', 0.5));
    expect(cr).toBeGreaterThan(cb);
    const [xr, , xb] = channels(heatColor('#2e8bff', 0.5));
    expect(xb).toBeGreaterThan(xr);
  });

  it('clamps out-of-range fill and tolerates a bad hex (returns the input)', () => {
    expect(luminance(heatColor('#ff7a18', 5))).toBeCloseTo(luminance(heatColor('#ff7a18', 1)), 5);
    expect(luminance(heatColor('#ff7a18', -2))).toBeCloseTo(luminance(heatColor('#ff7a18', 0)), 5);
    expect(heatColor('not-a-hex', 0.5)).toBe('not-a-hex');
  });
});
