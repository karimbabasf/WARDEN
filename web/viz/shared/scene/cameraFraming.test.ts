import { describe, it, expect } from 'vitest';
import {
  channelShiftPx,
  channelWidth,
  enclosingBox,
  frameDistance,
  pixelsToWorld,
  subtreeBounds,
} from './cameraFraming';
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

describe('channelWidth', () => {
  const RAIL = 300; // a plausible clamp(268px, 21vw, 340px) rail

  it('is the full canvas when no chrome is reserved', () => {
    expect(channelWidth(1600, { left: 0, right: 0 })).toBe(1600);
  });

  it('subtracts each reserved rail', () => {
    expect(channelWidth(1600, { left: RAIL, right: 0 })).toBe(1300);
    expect(channelWidth(1600, { left: RAIL, right: RAIL })).toBe(1000);
  });

  it('pulls the camera back once the rails squeeze the channel narrower than tall', () => {
    // 1100 wide minus two 300px rails leaves a 500x900 channel: portrait, so the
    // HORIZONTAL frustum is now the tight one and the fit has to retreat.
    const height = 900;
    const full = frameDistance(6, 46, 0.72, 1100 / height);
    const channel = frameDistance(6, 46, 0.72, channelWidth(1100, { left: RAIL, right: RAIL }) / height);
    expect(channel).toBeGreaterThan(full);
    // Exactly 1/aspect further out: tan(hFov/2) = tan(vFov/2) * aspect.
    const narrow = 500 / height;
    expect(channel).toBeCloseTo(frameDistance(6, 46, 0.72) / narrow, 10);
  });

  it('keeps the vertical-fov clamp correct: a still-wide channel needs no retreat', () => {
    // PerspectiveCamera.fov is VERTICAL only, so a bounding sphere that already fits
    // the height fits any channel wider than the height. 1600x900 minus two 300px
    // rails is 1000x900, still landscape, so pulling back there would just waste frame.
    const height = 900;
    const aspect = channelWidth(1600, { left: RAIL, right: RAIL }) / height;
    expect(aspect).toBeGreaterThan(1);
    expect(frameDistance(6, 46, 0.72, aspect)).toBeCloseTo(frameDistance(6, 46, 0.72), 10);
  });

  it('never returns a width that would divide the aspect to zero', () => {
    expect(channelWidth(400, { left: RAIL, right: RAIL })).toBe(1); // rails wider than the window
    expect(channelWidth(Number.NaN, { left: RAIL, right: 0 })).toBe(1);
    expect(channelWidth(1600, { left: Number.NaN, right: -50 })).toBe(1600); // junk insets ignored
  });
});

describe('channelShiftPx', () => {
  it('is zero with no chrome, and zero with symmetric chrome', () => {
    expect(channelShiftPx({ left: 0, right: 0 })).toBe(0);
    expect(channelShiftPx({ left: 300, right: 300 })).toBe(0);
  });

  it('pulls the camera LEFT when only the left rail is mounted', () => {
    // Camera left ⇒ scene right ⇒ the board clears the panel it was hiding under.
    expect(channelShiftPx({ left: 300, right: 0 })).toBe(-150);
  });

  it('pushes the camera RIGHT when the inspector opens on the right', () => {
    expect(channelShiftPx({ left: 0, right: 300 })).toBe(150);
  });

  it('is half the imbalance: it centres the scene in the free channel', () => {
    // Channel [340, 1600] has midpoint 970, canvas midpoint 800: the scene must appear
    // 170px right, so the camera moves 170px left.
    expect(channelShiftPx({ left: 340, right: 0 })).toBe(-170);
  });

  it('ignores junk insets rather than trucking the camera into nowhere', () => {
    expect(channelShiftPx({ left: Number.NaN, right: 300 })).toBe(150);
    expect(channelShiftPx({ left: -400, right: 0 })).toBe(0);
  });
});

describe('pixelsToWorld', () => {
  it('converts using the VERTICAL frustum height, never the width', () => {
    const d = 20, fov = 46, h = 900;
    const frustumHeight = 2 * d * Math.tan(((fov * Math.PI) / 180) / 2);
    expect(pixelsToWorld(h, d, fov, h)).toBeCloseTo(frustumHeight, 10); // a full screen height
    expect(pixelsToWorld(h / 2, d, fov, h)).toBeCloseTo(frustumHeight / 2, 10);
  });

  it('scales with distance: the same pixel offset is more world further out', () => {
    expect(pixelsToWorld(150, 40, 46, 900)).toBeCloseTo(pixelsToWorld(150, 20, 46, 900) * 2, 10);
  });

  it('keeps the sign, so a leftward shift stays leftward', () => {
    expect(pixelsToWorld(-150, 20, 46, 900)).toBeCloseTo(-pixelsToWorld(150, 20, 46, 900), 10);
    expect(pixelsToWorld(0, 20, 46, 900)).toBe(0);
  });

  it('returns 0 for a degenerate frame instead of NaN', () => {
    for (const badHeight of [0, -10, Number.NaN]) {
      expect(pixelsToWorld(150, 20, 46, badHeight)).toBe(0);
    }
    expect(pixelsToWorld(Number.NaN, 20, 46, 900)).toBe(0);
    expect(pixelsToWorld(150, Number.NaN, 46, 900)).toBe(0);
  });
});

describe('enclosingBox', () => {
  it('spans every node surface, not just the centres', () => {
    const box = enclosingBox([
      { pos: [0, 0, 0], radius: 1 },
      { pos: [10, -4, 0], radius: 2 },
    ])!;
    expect(box.min).toEqual([-1, -6, -2]);
    expect(box.max).toEqual([12, 1, 2]);
  });

  it('is null for an empty forest', () => {
    expect(enclosingBox([])).toBeNull();
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
