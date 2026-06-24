import { describe, expect, it } from 'vitest';
import {
  reconcileLifecycle,
  crossfadeFactor,
  isVisible,
  pruneGone,
  type LifecycleMap,
  type LiveId,
} from './radarLifecycle';

const DT = 1 / 60;

// Advance the reconciler N frames against a fixed set of live ids.
function run(prev: LifecycleMap, ids: LiveId[], frames: number, dt = DT): LifecycleMap {
  let map = prev;
  for (let i = 0; i < frames; i++) map = reconcileLifecycle(map, ids, dt);
  return map;
}

function live(id: string, status: 'working' | 'idle' | 'closed' = 'working'): LiveId {
  return { id, status };
}

describe('reconcileLifecycle — spawn', () => {
  it('a new id starts spawning at a small scale and grows toward 1', () => {
    const first = reconcileLifecycle({}, [live('a')], DT);
    expect(first.a.phase).toBe('spawning');
    expect(first.a.scale).toBeLessThan(1);
    expect(first.a.scale).toBeGreaterThanOrEqual(0);

    const later = run(first, [live('a')], 60);
    expect(later.a.scale).toBeGreaterThan(first.a.scale); // grew
    expect(later.a.scale).toBeLessThanOrEqual(1.0001);
  });

  it('transitions spawning → alive once it has essentially reached full scale', () => {
    const map = run({}, [live('a')], 240); // plenty of frames to settle
    expect(map.a.phase).toBe('alive');
    expect(map.a.scale).toBeGreaterThan(0.98);
  });
});

describe('reconcileLifecycle — alive', () => {
  it('an alive id whose fill changes stays alive at full scale (no snapping out)', () => {
    let map = run({}, [live('a', 'working')], 240);
    expect(map.a.phase).toBe('alive');
    // status flips working↔idle but the agent is still present → stays alive
    map = run(map, [live('a', 'idle')], 30);
    expect(map.a.phase).toBe('alive');
    expect(map.a.scale).toBeGreaterThan(0.98);
  });
});

describe('reconcileLifecycle — implode', () => {
  it('a disappearing id implodes: scale shrinks toward 0 then goes gone', () => {
    const alive = run({}, [live('a')], 240);
    expect(alive.a.phase).toBe('alive');

    const imploding = reconcileLifecycle(alive, [], DT); // id vanished
    expect(imploding.a.phase).toBe('imploding');
    expect(imploding.a.scale).toBeLessThan(alive.a.scale);

    const gone = run(imploding, [], 240);
    // once collapsed it is dropped from the map (or marked gone with ~0 scale)
    if (gone.a) {
      expect(gone.a.phase).toBe('gone');
      expect(gone.a.scale).toBeLessThan(0.02);
    } else {
      expect(gone.a).toBeUndefined();
    }
  });

  it('a present-but-closed id also implodes (status closed == ended)', () => {
    const alive = run({}, [live('a', 'working')], 240);
    const closing = reconcileLifecycle(alive, [live('a', 'closed')], DT);
    expect(closing.a.phase).toBe('imploding');
  });

  it('a re-appearing id during implosion grows back (no snapping)', () => {
    const alive = run({}, [live('a')], 240);
    const imploding = run(alive, [], 6); // started collapsing
    expect(imploding.a.phase).toBe('imploding');
    const reborn = reconcileLifecycle(imploding, [live('a')], DT);
    expect(reborn.a.phase).toBe('spawning');
  });
});

describe('reconcileLifecycle — smoothness', () => {
  it('never snaps: per-frame scale change is bounded by dt', () => {
    let map = reconcileLifecycle({}, [live('a')], DT);
    for (let i = 0; i < 120; i++) {
      const prevScale = map.a?.scale ?? 0;
      const next = reconcileLifecycle(map, [live('a')], DT);
      const delta = Math.abs((next.a?.scale ?? 0) - prevScale);
      expect(delta).toBeLessThan(0.2); // a single 1/60s step can't jump the whole way
      map = next;
    }
  });

  it('handles many simultaneous spawns and removals without throwing', () => {
    const ids = Array.from({ length: 50 }, (_, i) => live(`n${i}`));
    let map = run({}, ids, 30);
    expect(Object.keys(map).length).toBe(50);
    // remove half
    map = run(map, ids.slice(0, 25), 30);
    for (let i = 25; i < 50; i++) {
      expect(['imploding', 'gone']).toContain(map[`n${i}`]?.phase ?? 'gone');
    }
  });
});

describe('pruneGone', () => {
  it('drops fully-collapsed (gone) entries from the map', () => {
    const map: LifecycleMap = {
      alive: { phase: 'alive', t: 1, scale: 1 },
      imploding: { phase: 'imploding', t: 0.2, scale: 0.4 },
      gone: { phase: 'gone', t: 0.5, scale: 0 },
    };
    const pruned = pruneGone(map);
    expect(pruned.gone).toBeUndefined(); // gone node is dropped from the map
    expect(pruned.alive).toBe(map.alive); // survivors kept (identity preserved)
    expect(pruned.imploding).toBe(map.imploding); // mid-collapse still mounted
    expect(Object.keys(pruned)).toEqual(['alive', 'imploding']);
  });

  it('returns a new map and never mutates the input', () => {
    const map: LifecycleMap = { gone: { phase: 'gone', t: 0, scale: 0 } };
    const pruned = pruneGone(map);
    expect(pruned).not.toBe(map);
    expect(map.gone).toBeDefined(); // input untouched
    expect(Object.keys(pruned)).toHaveLength(0);
  });

  it('a closed id that has gone stays dropped after prune (no resurrection bloom)', () => {
    // alive → closed → fully imploded → pruned; feeding it closed again must keep
    // it gone (it must NOT bloom back to scale 1).
    const alive = run({}, [live('a', 'working')], 240);
    const gone = run(alive, [live('a', 'closed')], 240);
    expect(gone.a.phase).toBe('gone');
    const pruned = pruneGone(gone);
    expect(pruned.a).toBeUndefined();
    // next frame still reports the agent as closed (it lingers in model.agents)
    const after = reconcileLifecycle(pruned, [live('a', 'closed')], DT);
    expect(after.a.phase).toBe('gone');
    expect(after.a.scale).toBe(0); // did not resurrect to 1
  });
});

describe('isVisible', () => {
  it('reports a node renderable until it has fully gone', () => {
    expect(isVisible({ phase: 'spawning', t: 0.1, scale: 0.1 })).toBe(true);
    expect(isVisible({ phase: 'alive', t: 1, scale: 1 })).toBe(true);
    expect(isVisible({ phase: 'imploding', t: 0.5, scale: 0.3 })).toBe(true);
    expect(isVisible({ phase: 'gone', t: 1, scale: 0 })).toBe(false);
    expect(isVisible(undefined)).toBe(true); // unknown → treat as visible (full scale)
  });
});

describe('crossfadeFactor — tab switch', () => {
  it('eases 0→1 toward the active tab, bounded per frame (no snapping)', () => {
    let f = 0;
    const stepped = crossfadeFactor(f, 1, DT);
    expect(stepped).toBeGreaterThan(0);
    expect(stepped).toBeLessThan(1);
    for (let i = 0; i < 200; i++) f = crossfadeFactor(f, 1, DT);
    expect(f).toBeGreaterThan(0.98);
  });

  it('moves back toward 0 when the target flips', () => {
    let f = 1;
    f = crossfadeFactor(f, 0, DT);
    expect(f).toBeLessThan(1);
  });
});
