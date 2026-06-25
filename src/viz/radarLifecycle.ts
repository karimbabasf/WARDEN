// radarLifecycle.ts — the PURE lifecycle reconciler for radar globes.
//
// Smoothness is a hard requirement (spec §8): nothing ever snaps. This module is
// the single source of per-globe scale, derived frame-by-frame from a damped tween:
//
//   • a NEW agent → `spawning`, scale eased 0 → 1 (it blooms / a moon emerges).
//   • an agent still present → `alive` at full scale (fill/heat tween elsewhere).
//   • a vanished or `status:'closed'` agent → `imploding`, scale eased → 0
//     (collapse-into-self), then `gone` and dropped from the map.
//   • a re-appearing agent mid-implosion → back to `spawning` (grows again, no snap).
//
// It is a reducer over (prevMap, liveIds, dt) — zero Three.js — so the whole
// spawn→grow→implode behaviour is unit-tested deterministically. `RadarConstellation`
// multiplies each mesh's scale by the entry's `scale`. `crossfadeFactor` is the
// equally-pure tab cross-fade (one overlay, two scenes).

import { dampValue } from './useOrbCamera';

export type LifecyclePhase = 'spawning' | 'alive' | 'imploding' | 'gone';

export type LifecycleEntry = {
  phase: LifecyclePhase;
  /** Seconds spent in the current phase (for staging secondary effects). */
  t: number;
  /** 0..1 render scale — the load-bearing smoothness value. */
  scale: number;
};

export type LifecycleMap = Record<string, LifecycleEntry>;

/** The live forest's id + status, as fed each frame from the radar model. */
export type LiveId = { id: string; status: 'working' | 'idle' | 'closed' | 'terminated' };

// Tween rates (per second, exp-damped → inherently dt-bounded, never overshoot).
const SPAWN_LAMBDA = 7;
const IMPLODE_LAMBDA = 16;
const ALIVE_AT = 0.985; // spawning promotes to alive past this scale
const GONE_AT = 0.025; // imploding finishes (→ gone, then dropped) below this scale
const CROSSFADE_LAMBDA = 6;

/**
 * Fold one frame of the live forest into the lifecycle map, returning a NEW map.
 * `dt` is the frame delta in seconds (clamp upstream for tab-away spikes).
 */
export function reconcileLifecycle(prev: LifecycleMap, live: LiveId[], dt: number): LifecycleMap {
  const next: LifecycleMap = {};
  const liveById = new Map(live.map((l) => [l.id, l]));

  // 1) Every live (non-closed) id: spawn or stay alive.
  for (const { id, status } of live) {
    const was = prev[id];
    // `closed` (root/process gone) and `terminated` (a finished subagent) are both
    // terminal: implode once, then stay gone (no resurrection bloom).
    const closed = status === 'closed' || status === 'terminated';

    if (closed) {
      // A closed id whose gone entry was already pruned (or that first appears
      // closed) must NOT bloom back to scale 1 just to implode again — it is dead.
      // Stay gone so `pruneGone` keeps it dropped (no resurrection flicker).
      if (!was || was.phase === 'gone') {
        next[id] = { phase: 'gone', t: 0, scale: 0 };
        continue;
      }
      // Present but ended → implode (handled in the same shrink path as vanished).
      const scale = dampValue(was.scale, 0, IMPLODE_LAMBDA, dt);
      if (scale <= GONE_AT) {
        next[id] = { phase: 'gone', t: was.t + dt, scale: 0 };
      } else {
        next[id] = { phase: 'imploding', t: was.phase === 'imploding' ? was.t + dt : 0, scale };
      }
      continue;
    }

    if (!was || was.phase === 'imploding' || was.phase === 'gone') {
      // brand new, or caught mid-implosion and reborn → (re)spawn from current scale
      const startScale = was ? was.scale : 0;
      const scale = dampValue(startScale, 1, SPAWN_LAMBDA, dt);
      next[id] = { phase: 'spawning', t: 0, scale };
      continue;
    }

    if (was.phase === 'spawning') {
      const scale = dampValue(was.scale, 1, SPAWN_LAMBDA, dt);
      next[id] =
        scale >= ALIVE_AT
          ? { phase: 'alive', t: 0, scale: 1 }
          : { phase: 'spawning', t: was.t + dt, scale };
      continue;
    }

    // already alive → hold full scale (heat/position tween lives in the render)
    next[id] = { phase: 'alive', t: was.t + dt, scale: 1 };
  }

  // 2) Ids present last frame but no longer live → implode then drop.
  for (const id in prev) {
    if (liveById.has(id)) continue; // handled above
    const was = prev[id];
    if (was.phase === 'gone') continue; // already finished → let it fall out of the map
    const scale = dampValue(was.scale, 0, IMPLODE_LAMBDA, dt);
    if (scale <= GONE_AT) {
      next[id] = { phase: 'gone', t: was.t + dt, scale: 0 };
    } else {
      next[id] = { phase: 'imploding', t: was.phase === 'imploding' ? was.t + dt : 0, scale };
    }
  }

  return next;
}

/** A node is renderable until it has fully collapsed (`gone`). Unknown = visible. */
export function isVisible(entry: LifecycleEntry | undefined): boolean {
  if (!entry) return true;
  return entry.phase !== 'gone';
}

/**
 * Drop every fully-collapsed (`gone`) entry, returning a NEW map. A `gone` globe
 * has finished imploding — keeping it lingers an invisible (scale 0) node + its
 * hit-sphere until the next model emit. Pruning it here unmounts it promptly. Pure
 * (no Three.js) so the prune is unit-tested. Imploding/spawning/alive entries are
 * preserved untouched so their animation keeps playing.
 */
export function pruneGone(map: LifecycleMap): LifecycleMap {
  const next: LifecycleMap = {};
  for (const id in map) {
    if (map[id].phase !== 'gone') next[id] = map[id];
  }
  return next;
}

/**
 * Damp a cross-fade factor toward its target (1 = show this scene, 0 = hide it).
 * Pure + dt-bounded so a tab switch fades the two constellations, never cuts.
 */
export function crossfadeFactor(current: number, target: 0 | 1, dt: number): number {
  return dampValue(current, target, CROSSFADE_LAMBDA, dt);
}

/** Snapshot overlay should unmount once the incoming scene is essentially in. */
const OVERLAY_DONE_AT = 0.985;

export type CrossfadeOverlay = {
  /** Opacity of the OUTGOING frozen snapshot, faded over the live incoming scene. */
  opacity: number;
  /** Keep the snapshot layer mounted? Drops it once the fade is essentially done. */
  mounted: boolean;
};

/**
 * Map a tab-transition `progress` (0 = just switched, 1 = incoming fully faded in;
 * drive it with `crossfadeFactor`) to the OUTGOING snapshot overlay's render state.
 *
 * The live <Canvas> swaps to the incoming constellation IMMEDIATELY on a tab change
 * (single warm mount, never remounted); a frozen frame of the outgoing scene is held
 * as a DOM layer ON TOP at full opacity, then dissolved away as `progress` climbs —
 * so the eye sees the old constellation cross-fade into the new one, never a hard cut
 * (spec §8). Opacity is the complement of progress, clamped so a noisy/NaN input can
 * never yield a stuck or invalid overlay. Pure ⇒ unit-tested without WebGL.
 */
export function crossfadeOverlay(progress: number): CrossfadeOverlay {
  const p = Number.isFinite(progress) ? Math.max(0, Math.min(1, progress)) : 0;
  return { opacity: 1 - p, mounted: p < OVERLAY_DONE_AT };
}
