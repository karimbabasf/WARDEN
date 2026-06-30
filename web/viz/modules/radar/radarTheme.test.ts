import { describe, expect, it } from 'vitest';
import { RADAR_PALETTE, radarHarness } from './radarTheme';

function channels(hex: string): [number, number, number] {
  const n = parseInt(/^#?([0-9a-f]{6})$/i.exec(hex.trim())![1], 16);
  return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
}

describe('RADAR_PALETTE', () => {
  it('is two distinct brand hues: Claude tangerine + Codex cyan-ice', () => {
    expect(RADAR_PALETTE.claude_code.color).toBe('#ff8636');
    expect(RADAR_PALETTE.codex.color).toBe('#4fc9ff');
  });

  it('Claude reads warm (red dominant), Codex reads cool (blue dominant)', () => {
    const [cr, , cb] = channels(RADAR_PALETTE.claude_code.color);
    expect(cr).toBeGreaterThan(cb);
    const [xr, , xb] = channels(RADAR_PALETTE.codex.color);
    expect(xb).toBeGreaterThan(xr);
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
    expect(radarHarness('codex').color).toBe('#4fc9ff');
    expect(radarHarness('claude_code').color).toBe('#ff8636');
  });

  it('falls back to a neutral chip for unknown harnesses', () => {
    const n = radarHarness('gemini');
    expect(n.label).toBe('Unknown');
    expect(n.color).not.toBe('#ff8636');
    expect(n.color).not.toBe('#4fc9ff');
    expect(n.glyph.length).toBeGreaterThan(0);
  });
});
