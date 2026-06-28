import { describe, expect, it } from 'vitest';
import { HEATMAP_LEVELS, heatmapFill, heatmapLevel, maxTokens } from './heatmap';

describe('heatmapLevel', () => {
  it('treats zero / negative / non-finite totals as the empty level 0', () => {
    expect(heatmapLevel(0, 1000)).toBe(0);
    expect(heatmapLevel(-5, 1000)).toBe(0);
    expect(heatmapLevel(Number.NaN, 1000)).toBe(0);
    expect(heatmapLevel(Number.POSITIVE_INFINITY, 1000)).toBe(0);
  });

  it('puts the busiest day at the top level and a tiny day at level 1', () => {
    const max = 10_000;
    expect(heatmapLevel(max, max)).toBe(HEATMAP_LEVELS - 1); // 4
    expect(heatmapLevel(1, max)).toBe(1); // smallest positive → level 1, never 0
  });

  it('is monotonic non-decreasing in tokens', () => {
    const max = 10_000;
    let prev = -1;
    for (const t of [0, 100, 1000, 2500, 5000, 7500, 9999, 10_000]) {
      const lvl = heatmapLevel(t, max);
      expect(lvl).toBeGreaterThanOrEqual(prev);
      prev = lvl;
    }
  });

  it('keeps every level within 0..LEVELS-1', () => {
    const max = 8_000;
    for (const t of [0, 1, 500, 4000, 8000, 20_000]) {
      const lvl = heatmapLevel(t, max);
      expect(lvl).toBeGreaterThanOrEqual(0);
      expect(lvl).toBeLessThanOrEqual(HEATMAP_LEVELS - 1);
    }
  });

  it('clamps tokens above the window max to the top level (no overflow)', () => {
    expect(heatmapLevel(50_000, 10_000)).toBe(HEATMAP_LEVELS - 1);
  });

  it('shows an active day at full intensity when the window max is unknown / zero', () => {
    expect(heatmapLevel(123, 0)).toBe(HEATMAP_LEVELS - 1);
    expect(heatmapLevel(123, Number.NaN)).toBe(HEATMAP_LEVELS - 1);
  });

  it('spreads a mid-volume day into a middle level, not the extremes', () => {
    const lvl = heatmapLevel(5_000, 10_000);
    expect(lvl).toBeGreaterThan(0);
    expect(lvl).toBeLessThan(HEATMAP_LEVELS - 1);
  });
});

describe('heatmapFill', () => {
  it('returns a distinct colour string for each level', () => {
    const fills = [0, 1, 2, 3, 4].map(heatmapFill);
    expect(new Set(fills).size).toBe(5);
    fills.forEach((f) => expect(typeof f).toBe('string'));
  });

  it('falls back to the hot colour for out-of-range levels', () => {
    expect(heatmapFill(99)).toBe(heatmapFill(4));
  });
});

describe('maxTokens', () => {
  it('returns the largest total_tokens, ignoring non-finite values', () => {
    expect(
      maxTokens([
        { total_tokens: 100 },
        { total_tokens: 9000 },
        { total_tokens: Number.NaN },
        { total_tokens: 42 },
      ]),
    ).toBe(9000);
  });

  it('returns 0 for an empty list', () => {
    expect(maxTokens([])).toBe(0);
  });
});
