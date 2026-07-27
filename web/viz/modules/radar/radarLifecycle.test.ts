import { describe, expect, it } from 'vitest';
import {
  reconcileLifecycle,
  crossfadeFactor,
  crossfadeOverlay,
  isVisible,
  linkDrawProgress,
  pruneGone,
  type LifecycleEntry,
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

function live(id: string, status: 'working' | 'idle' | 'closed' | 'terminated' = 'working'): LiveId {
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

describe('reconcileLifecycle — terminated subagent', () => {
  it('a terminated id implodes like closed and does not resurrect after prune', () => {
    const alive = run({}, [live('a', 'working')], 240);
    const closing = reconcileLifecycle(alive, [live('a', 'terminated')], DT);
    expect(closing.a.phase).toBe('imploding');

    const gone = run(closing, [live('a', 'terminated')], 240);
    expect(gone.a.phase).toBe('gone');
    const pruned = pruneGone(gone);
    expect(pruned.a).toBeUndefined();
    // a stray late payload still tagging it terminated must NOT bloom it back
    const after = reconcileLifecycle(pruned, [live('a', 'terminated')], DT);
    expect(after.a.phase).toBe('gone');
    expect(after.a.scale).toBe(0);
  });

  it('finishes a terminated subagent collapse in under half a second', () => {
    const alive = run({}, [live('a', 'working')], 240);
    const gone = run(alive, [live('a', 'terminated')], 24);

    expect(gone.a.phase).toBe('gone');
    expect(gone.a.scale).toBe(0);
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

describe('linkDrawProgress — the tether draws out and is reeled back in', () => {
  const alive: LifecycleEntry = { phase: 'alive', t: 1, scale: 1 };
  const entry = (phase: LifecycleEntry['phase'], scale: number): LifecycleEntry => ({ phase, t: 0.1, scale });

  it('is fully drawn between two settled globes', () => {
    expect(linkDrawProgress({ entry: alive }, { entry: alive })).toBe(1);
    expect(linkDrawProgress({}, {})).toBe(1); // no lifecycle entry yet = a live link
  });

  it('draws OUT from the parent as the child blooms (monotonic 0 → 1)', () => {
    const at = (s: number) => linkDrawProgress({ entry: alive }, { entry: entry('spawning', s) });
    expect(at(0)).toBe(0);
    let last = 0;
    for (const s of [0.1, 0.25, 0.4, 0.6, 0.75]) {
      const p = at(s);
      expect(p).toBeGreaterThan(last);
      expect(p).toBeLessThanOrEqual(1);
      last = p;
    }
  });

  it('ARRIVES just before the child finishes blooming, not chasing its damped tail', () => {
    // Fully home at 82% of the bloom, so the line lands as the globe settles rather
    // than creeping the last few percent for another half second.
    expect(linkDrawProgress({ entry: alive }, { entry: entry('spawning', 0.82) })).toBeCloseTo(1, 10);
    expect(linkDrawProgress({ entry: alive }, { entry: entry('spawning', 0.9) })).toBe(1);
    expect(linkDrawProgress({ entry: alive }, { entry: entry('spawning', 0.6) })).toBeLessThan(1);
  });

  it('RETRACTS slightly ahead of the imploding child, so nothing is left dangling', () => {
    for (const s of [0.2, 0.5, 0.8]) {
      const p = linkDrawProgress({ entry: alive }, { entry: entry('imploding', s) });
      expect(p).toBeLessThan(s); // the line is already shorter than the globe is small
      expect(p).toBeGreaterThan(0);
    }
    expect(linkDrawProgress({ entry: alive }, { entry: entry('imploding', 0) })).toBe(0);
  });

  it('retracting is monotonic: the tether never lengthens on its way home', () => {
    let last = Infinity;
    for (const s of [0.9, 0.7, 0.5, 0.3, 0.1, 0]) {
      const p = linkDrawProgress({ entry: alive }, { entry: entry('imploding', s) });
      expect(p).toBeLessThan(last);
      last = p;
    }
  });

  it('drops to zero the moment either endpoint is gone or pruned', () => {
    expect(linkDrawProgress({ entry: alive }, { entry: entry('gone', 0) })).toBe(0);
    expect(linkDrawProgress({ entry: entry('gone', 0) }, { entry: alive })).toBe(0);
    expect(linkDrawProgress({ gone: true }, { entry: alive })).toBe(0);
    expect(linkDrawProgress({ entry: alive }, { gone: true })).toBe(0);
  });

  it('is never longer than the shorter endpoint allows (a vanishing parent wins)', () => {
    const p = linkDrawProgress({ entry: entry('imploding', 0.2) }, { entry: alive });
    expect(p).toBeLessThan(0.2);
  });

  it('is identical for any two endpoints in the same lifecycle state (no harness branch)', () => {
    // Claude agent-teams and Codex subagents are different upstream concepts; the
    // animation may not know the difference, and this function takes no harness at all.
    const claudeLike = linkDrawProgress({ entry: alive }, { entry: entry('spawning', 0.5) });
    const codexLike = linkDrawProgress({ entry: alive }, { entry: entry('spawning', 0.5) });
    expect(claudeLike).toBe(codexLike);
  });

  it('never snaps: a 1/60s step of the reconciler moves it by a bounded amount', () => {
    // Driven by the real reconciler, from nothing, at a real frame delta: no step of
    // the draw may jump a large part of the link, at either end of the lifecycle.
    let map: LifecycleMap = {};
    let prev = 0;
    for (let i = 0; i < 120; i++) {
      map = reconcileLifecycle(map, [live('a')], DT);
      const p = linkDrawProgress({ entry: alive }, { entry: map.a });
      expect(p - prev).toBeGreaterThanOrEqual(0); // draws out, never flickers back
      expect(p - prev).toBeLessThan(0.2);
      prev = p;
    }
    expect(prev).toBe(1);

    for (let i = 0; i < 120; i++) {
      map = reconcileLifecycle(map, [], DT); // the child vanished: reel it back in
      // The reconciler drops the entry the frame AFTER it goes: past that point the
      // globe has unmounted and there is no link left to draw, so stop measuring.
      if (!map.a) break;
      const p = linkDrawProgress({ entry: alive }, { entry: map.a });
      expect(p - prev).toBeLessThanOrEqual(0); // retracts, never lengthens on the way out
      // The implosion itself is deliberately quick (IMPLODE_LAMBDA is more than twice
      // SPAWN_LAMBDA), so the retract is allowed a correspondingly larger step: it has
      // to stay with the globe, not lag behind it leaving a stub.
      expect(prev - p).toBeLessThan(0.3);
      prev = p;
    }
    expect(prev).toBe(0);
  });

  it('clamps a corrupt scale instead of emitting NaN or a negative length', () => {
    for (const bad of [Number.NaN, -3, 42, Number.POSITIVE_INFINITY]) {
      const p = linkDrawProgress({ entry: alive }, { entry: entry('spawning', bad) });
      expect(Number.isFinite(p)).toBe(true);
      expect(p).toBeGreaterThanOrEqual(0);
      expect(p).toBeLessThanOrEqual(1);
    }
  });

  it('reduced motion: one clean flip at the halfway mark, never a travelling draw', () => {
    const at = (phase: LifecycleEntry['phase'], s: number) =>
      linkDrawProgress({ entry: alive }, { entry: entry(phase, s) }, true);
    // only ever fully drawn or fully absent
    for (const s of [0, 0.15, 0.3, 0.5, 0.7, 0.9]) {
      expect([0, 1]).toContain(at('spawning', s));
      expect([0, 1]).toContain(at('imploding', s));
    }
    expect(at('spawning', 0.1)).toBe(0); // early bloom: no line yet
    expect(at('spawning', 0.8)).toBe(1); // settled: line present
    expect(at('imploding', 0.2)).toBe(0); // going home: line already dropped
    expect(linkDrawProgress({ entry: alive }, { entry: alive }, true)).toBe(1);
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

describe('crossfadeOverlay — outgoing-snapshot fade', () => {
  it('is fully opaque + mounted at the start of the transition (progress 0)', () => {
    const o = crossfadeOverlay(0);
    expect(o.opacity).toBe(1);
    expect(o.mounted).toBe(true);
  });

  it('fades the snapshot opacity down as the incoming scene fades in', () => {
    expect(crossfadeOverlay(0.25).opacity).toBeGreaterThan(crossfadeOverlay(0.75).opacity);
    // opacity is the complement of progress (incoming in ⇒ outgoing out)
    expect(crossfadeOverlay(0.4).opacity).toBeCloseTo(0.6, 5);
  });

  it('unmounts the snapshot once the fade is essentially complete (no lingering layer)', () => {
    expect(crossfadeOverlay(1).mounted).toBe(false);
    expect(crossfadeOverlay(0.999).mounted).toBe(false); // within epsilon → dropped
    expect(crossfadeOverlay(0.9).mounted).toBe(true); // still mid-fade → kept
  });

  it('clamps a noisy progress value into a valid [0,1] opacity', () => {
    expect(crossfadeOverlay(-0.5).opacity).toBe(1);
    expect(crossfadeOverlay(1.5).opacity).toBe(0);
    expect(crossfadeOverlay(Number.NaN).opacity).toBe(1); // defensive: never NaN opacity
  });
});
