import { describe, expect, it } from 'vitest';
import {
  peerLayoutBounds,
  peerLayoutBox,
  peerOffsetX,
  peerWorldPlacement,
  unionBounds,
} from './peerFraming';
import type { Bounds } from '@/viz/shared/scene/cameraFraming';
import type { RadarAgent, RadarSceneModel } from '@/viz/shared/types/radarTypes';

function agent(partial: Partial<RadarAgent> & Pick<RadarAgent, 'id'>): RadarAgent {
  return {
    harness: 'claude_code',
    origin: null,
    parentId: null,
    depth: 0,
    label: partial.id,
    nickname: null,
    repo: null,
    cwd: '~/work',
    role: null,
    model: null,
    status: 'working',
    contextTokens: 40_000,
    maxTokens: 200_000,
    fillPct: 0.2,
    composition: { exact: { cacheRead: 0, fresh: 0, output: 0 }, estimated: null },
    recentActivity: [],
    childCount: 0,
    startedAt: '',
    estCostUsd: null,
    ...partial,
  } as RadarAgent;
}

/**
 * A board of `roots` sessions, each carrying `subs` subagents. `folders` controls how
 * many project rails they spread across, since the layout stacks one rail per folder:
 * one folder is a wide board, one folder per root is a tall one.
 */
function board(roots: number, subs = 0, prefix = 'a', folders = roots): RadarSceneModel {
  const agents: RadarAgent[] = [];
  for (let r = 0; r < roots; r++) {
    const id = `${prefix}-root-${r}`;
    agents.push(agent({ id, cwd: `~/proj-${r % Math.max(1, folders)}` }));
    for (let s = 0; s < subs; s++) {
      agents.push(agent({ id: `${id}-sub-${s}`, parentId: id, depth: 1, contextTokens: 8_000 }));
    }
  }
  return { generatedAt: 'T', agents };
}

const sphere = (x: number, radius: number): Bounds => ({ center: [x, 0, 0], radius });

/** Do two bounding spheres touch or intersect? */
function overlaps(a: Bounds, b: Bounds): boolean {
  const d = Math.hypot(
    a.center[0] - b.center[0],
    a.center[1] - b.center[1],
    a.center[2] - b.center[2],
  );
  return d <= a.radius + b.radius;
}

describe('peerOffsetX', () => {
  it('parks the peer to the RIGHT of the local board', () => {
    expect(peerOffsetX(sphere(0, 5), sphere(0, 5))).toBeGreaterThan(0);
  });

  it('leaves clear air between the two: they never touch', () => {
    const local = sphere(0, 5);
    const peer = sphere(0, 3);
    const moved = sphere(peer.center[0] + peerOffsetX(local, peer), peer.radius);
    expect(overlaps(local, moved)).toBe(false);
  });

  it('scales the gap with the boards, so they cannot overlap AT ANY SIZE', () => {
    // The whole point of deriving the gap rather than picking a number: a constant
    // that clears two agents is swallowed whole by a forty-agent board.
    for (const localR of [0.5, 4, 25, 300]) {
      for (const peerR of [0.5, 7, 60, 500]) {
        const local = sphere(-3, localR);
        const peer = sphere(11, peerR);
        const moved = sphere(peer.center[0] + peerOffsetX(local, peer), peerR);
        expect(overlaps(local, moved)).toBe(false);
      }
    }
  });

  it('grows the gap as the boards grow (not a fixed distance)', () => {
    const small = peerOffsetX(sphere(0, 2), sphere(0, 2));
    const large = peerOffsetX(sphere(0, 40), sphere(0, 40));
    expect(large).toBeGreaterThan(small * 10);
  });

  it('is independent of where the peer happened to lay itself out', () => {
    const local = sphere(0, 6);
    const a = peerOffsetX(local, sphere(0, 4));
    const b = peerOffsetX(local, sphere(-250, 4));
    // Both land the peer's centre in the same world place, whatever its own origin.
    expect(-250 + b).toBeCloseTo(0 + a, 10);
  });

  it('handles an empty local board (first peer against nothing)', () => {
    const peer = sphere(0, 4);
    const moved = sphere(peer.center[0] + peerOffsetX(null, peer), peer.radius);
    expect(moved.center[0]).toBeGreaterThan(peer.radius); // clear of the origin
  });

  it('is 0 when there is no peer at all', () => {
    expect(peerOffsetX(sphere(0, 5), null)).toBe(0);
  });
});

describe('peerWorldPlacement', () => {
  it('reports bounds that match the offset it hands the constellation', () => {
    const model = board(3, 2);
    const own = peerLayoutBounds(model)!;
    const placement = peerWorldPlacement(model, sphere(0, 9));
    expect(placement.bounds).not.toBeNull();
    expect(placement.bounds!.center[0]).toBeCloseTo(own.center[0] + placement.offsetX, 10);
    expect(placement.bounds!.radius).toBeCloseTo(own.radius, 10);
    // Y and Z are untouched: the slide is lateral only.
    expect(placement.bounds!.center[1]).toBeCloseTo(own.center[1], 10);
    expect(placement.bounds!.center[2]).toBeCloseTo(own.center[2], 10);
  });

  it('keeps two REAL boards apart, however lopsided their sizes', () => {
    const cases: Array<[RadarSceneModel, RadarSceneModel]> = [
      [board(1, 0, 'l'), board(1, 0, 'p')],
      [board(1, 0, 'l'), board(12, 4, 'p')],
      [board(12, 4, 'l'), board(1, 0, 'p')],
      [board(9, 3, 'l'), board(9, 3, 'p')],
    ];
    for (const [localModel, peerModel] of cases) {
      const localBounds = peerLayoutBounds(localModel)!;
      const placement = peerWorldPlacement(peerModel, localBounds);
      expect(overlaps(localBounds, placement.bounds!)).toBe(false);
    }
  });

  it('returns an empty placement for a missing or empty peer model', () => {
    expect(peerWorldPlacement(null, sphere(0, 5))).toEqual({ offsetX: 0, bounds: null });
    expect(peerWorldPlacement({ generatedAt: 'T', agents: [] }, sphere(0, 5))).toEqual({
      offsetX: 0,
      bounds: null,
    });
  });
});

describe('peerLayoutBox', () => {
  it('reports the real proportions, which a bounding sphere cannot', () => {
    // Five sessions on ONE rail is a wide, short board; five sessions on five rails is
    // a tall one. A frame drawn from the sphere radius would be square for both.
    const wide = peerLayoutBox(board(5, 2, 'w', 1))!;
    expect(wide.max[0] - wide.min[0]).toBeGreaterThan(wide.max[1] - wide.min[1]);

    const tall = peerLayoutBox(board(5, 2, 't', 5))!;
    expect(tall.max[1] - tall.min[1]).toBeGreaterThan(wide.max[1] - wide.min[1]);
  });

  it('encloses the bounding sphere it shares a layout with', () => {
    const model = board(4, 1);
    const box = peerLayoutBox(model)!;
    const bounds = peerLayoutBounds(model)!;
    // Every corner-to-centre reach is at least the sphere's, on each axis it spans.
    expect(box.max[0] - box.min[0]).toBeGreaterThan(0);
    expect(bounds.center[0]).toBeGreaterThanOrEqual(box.min[0]);
    expect(bounds.center[0]).toBeLessThanOrEqual(box.max[0]);
  });

  it('is null for an empty model', () => {
    expect(peerLayoutBox({ generatedAt: 'T', agents: [] })).toBeNull();
  });
});

describe('unionBounds', () => {
  it('contains both boards, so the far plane never clips one away', () => {
    const a = sphere(0, 4);
    const b = sphere(30, 6);
    const u = unionBounds(a, b)!;
    for (const s of [a, b]) {
      const d = Math.abs(u.center[0] - s.center[0]);
      expect(d + s.radius).toBeLessThanOrEqual(u.radius + 1e-9);
    }
  });

  it('returns the swallowing sphere when one already contains the other', () => {
    const big = sphere(0, 50);
    expect(unionBounds(big, sphere(3, 2))).toBe(big);
    expect(unionBounds(sphere(3, 2), big)).toBe(big);
  });

  it('passes through when either side is missing', () => {
    const a = sphere(0, 4);
    expect(unionBounds(a, null)).toBe(a);
    expect(unionBounds(null, a)).toBe(a);
    expect(unionBounds(null, null)).toBeNull();
  });
});
