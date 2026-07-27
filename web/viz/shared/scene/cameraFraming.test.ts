import { describe, it, expect } from 'vitest';
import { frameDistance, subtreeBounds } from './cameraFraming';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';

describe('frameDistance', () => {
  it('is monotonic: larger radius yields larger distance', () => {
    const d1 = frameDistance(1, 46);
    const d2 = frameDistance(2, 46);
    expect(d2).toBeGreaterThan(d1);
  });

  it('exact value at fov=46, r=2, fill=0.6', () => {
    const expected = 2 / (Math.tan(((46 * Math.PI) / 180) / 2) * 0.6);
    expect(frameDistance(2, 46, 0.6)).toBeCloseTo(expected, 10);
  });

  it('uses default fill=0.6 when fill is omitted', () => {
    const withDefault = frameDistance(2, 46);
    const explicit = frameDistance(2, 46, 0.6);
    expect(withDefault).toBeCloseTo(explicit, 10);
  });

  it('omitting aspect keeps the original vertical-only framing', () => {
    expect(frameDistance(2, 46, 0.6, undefined)).toBeCloseTo(frameDistance(2, 46, 0.6), 10);
  });

  it('a wide window (aspect > 1) is unchanged: the vertical frustum is the tight one', () => {
    expect(frameDistance(2, 46, 0.6, 1.9)).toBeCloseTo(frameDistance(2, 46, 0.6), 10);
  });

  it('a square window (aspect = 1) matches vertical-only framing', () => {
    expect(frameDistance(2, 46, 0.6, 1)).toBeCloseTo(frameDistance(2, 46, 0.6), 10);
  });

  it('a narrow window (aspect < 1) pulls the camera back so the width still fits', () => {
    const wide = frameDistance(2, 46, 0.6, 1.6);
    const narrow = frameDistance(2, 46, 0.6, 0.5);
    expect(narrow).toBeGreaterThan(wide);
    // Exactly 1/aspect further out: tan(hFov/2) = tan(vFov/2) * aspect.
    expect(narrow).toBeCloseTo(frameDistance(2, 46, 0.6) / 0.5, 10);
  });

  it('a degenerate aspect (0, NaN, negative) falls back to vertical framing, never NaN', () => {
    const base = frameDistance(2, 46, 0.6);
    for (const bad of [0, Number.NaN, -1, Number.POSITIVE_INFINITY]) {
      const d = frameDistance(2, 46, 0.6, bad);
      expect(Number.isFinite(d)).toBe(true);
      expect(d).toBeCloseTo(base, 10);
    }
  });
});

describe('subtreeBounds', () => {
  // Fixture: root -> childA, root -> childB
  // Each node at a known position with radius 0.5
  const agents: RadarAgent[] = [
    { id: 'root', parentId: null } as unknown as RadarAgent,
    { id: 'childA', parentId: 'root' } as unknown as RadarAgent,
    { id: 'childB', parentId: 'root' } as unknown as RadarAgent,
  ];

  const positions = new Map<string, { pos: [number, number, number]; radius: number }>([
    ['root', { pos: [0, 0, 0], radius: 0.5 }],
    ['childA', { pos: [2, 0, 0], radius: 0.5 }],
    ['childB', { pos: [-2, 0, 0], radius: 0.5 }],
  ]);

  it('encloses all three member centers: each member center within bounds.radius of bounds.center', () => {
    const bounds = subtreeBounds(positions, agents, 'root');

    for (const [id] of positions) {
      const member = positions.get(id)!;
      const [cx, cy, cz] = bounds.center;
      const [mx, my, mz] = member.pos;
      const dist = Math.sqrt(
        (mx - cx) ** 2 + (my - cy) ** 2 + (mz - cz) ** 2,
      );
      expect(dist).toBeLessThanOrEqual(bounds.radius + 1e-10);
    }
  });

  it('leaf root returns its own position and radius', () => {
    const leafAgents: RadarAgent[] = [
      { id: 'solo', parentId: null } as unknown as RadarAgent,
    ];
    const leafPositions = new Map([['solo', { pos: [3, 4, 5] as [number, number, number], radius: 1.2 }]]);
    const bounds = subtreeBounds(leafPositions, leafAgents, 'solo');
    expect(bounds.center).toEqual([3, 4, 5]);
    expect(bounds.radius).toBeCloseTo(1.2, 10);
  });

  it('skips ids absent from positions map', () => {
    // childB is NOT in the positions map — should not throw
    const partialPositions = new Map<string, { pos: [number, number, number]; radius: number }>([
      ['root', { pos: [0, 0, 0], radius: 0.5 }],
      ['childA', { pos: [1, 0, 0], radius: 0.5 }],
      // childB intentionally omitted
    ]);
    expect(() => subtreeBounds(partialPositions, agents, 'root')).not.toThrow();
    const bounds = subtreeBounds(partialPositions, agents, 'root');
    // Should still enclose root and childA
    for (const id of ['root', 'childA']) {
      const member = partialPositions.get(id)!;
      const [cx, cy, cz] = bounds.center;
      const [mx, my, mz] = member.pos;
      const dist = Math.sqrt((mx - cx) ** 2 + (my - cy) ** 2 + (mz - cz) ** 2);
      expect(dist).toBeLessThanOrEqual(bounds.radius + 1e-10);
    }
  });
});
