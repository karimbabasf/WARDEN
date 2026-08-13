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
// equally-pure tab cross-fade (one overlay, two scenes), and `linkDrawProgress` is
// the equally-pure draw/retract of a parent→child tether (see its own note below).

import { dampValue } from '@/viz/shared/scene/useOrbCamera';

export type LifecyclePhase = 'spawning' | 'alive' | 'imploding' | 'gone';

export type LifecycleEntry = {
  phase: LifecyclePhase;
  /** Seconds spent in the current phase (for staging secondary effects). */
  t: number;
  /** 0..1 render scale — the load-bearing smoothness value. */
  scale: number;
};

export type LifecycleMap = Record<string, LifecycleEntry>;

/**
 * The live forest's id + status, as fed each frame from the radar model.
 *
 * `awaiting` is a LIVE state here, not a terminal one: an agent blocked on a question is
 * very much still on the board, and the only thing the reconciler cares about is whether
 * a globe is leaving.
 */
export type LiveId = {
  id: string;
  status: 'working' | 'awaiting' | 'idle' | 'closed' | 'terminated';
};

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

// ── link draw / retract ───────────────────────────────────────────────────────
// A parent->child tether is not a static rod that blinks out with its child: it is
// DRAWN along its own length. `linkDrawProgress` is the fraction of the parent->child
// vector that is currently rendered (0 = fully retracted into the parent, 1 = fully
// drawn to the child), so the renderer redistributes its existing segments over
// [parent, parent + progress * (child - parent)] instead of building new geometry:
//
//   • spawn: the stroke reaches out FROM the parent and lands on the child just as
//     the child finishes blooming (it arrives, it does not fade up in place);
//   • despawn: the stroke is REELED BACK into the parent while the child implodes,
//     so the child is pulled home rather than orphaned at the end of a dead line.
//
// It is one pure value per link, derived from the same lifecycle entries the globes
// read, so the two are in lockstep by construction. Claude agent-teams and Codex
// subagents are different upstream concepts and deliberately share this exact path:
// nothing here may branch on harness.

/** One end of a link: its lifecycle entry, plus whether it was pruned as `gone`. */
export type LinkEndpointState = { entry?: LifecycleEntry; gone?: boolean };

// The stroke is fully home at 82% of the child's bloom, so the line ARRIVES a beat
// before the globe settles instead of chasing it forever down the damped tail.
const LINK_ARRIVE_AT = 0.82;
// Retract slightly AHEAD of the child's collapse (>1 exponent ⇒ progress < scale), so
// the last of the line is inside the parent by the time the globe reaches zero.
const LINK_RETRACT_EXP = 1.25;
// Reduced motion: one clean state flip at the halfway mark, never a travelling draw.
const LINK_INSTANT_AT = 0.5;

function clamp01(v: number): number {
  return Number.isFinite(v) ? Math.max(0, Math.min(1, v)) : 0;
}

/** How much of a link one endpoint permits to be drawn. */
function endpointDraw({ entry, gone = false }: LinkEndpointState): number {
  if (gone || entry?.phase === 'gone') return 0;
  if (!entry) return 1;
  const scale = clamp01(entry.scale);
  if (entry.phase === 'imploding') return Math.pow(scale, LINK_RETRACT_EXP);
  // The child's own bloom is already an exp-damped ease-out, so the draw rides it
  // LINEARLY: the head decelerates exactly as the globe settles, which is what puts
  // the two in lockstep. Easing on top of an ease would front-load the whole stroke
  // into the first few frames and read as a snap.
  if (entry.phase === 'spawning') return clamp01(scale / LINK_ARRIVE_AT);
  return 1;
}

/**
 * Drawn fraction of a parent->child link, 0..1. The shorter of the two endpoints
 * wins, so a link is never longer than either globe it hangs between.
 *
 * `instant` (prefers-reduced-motion) collapses the travelling draw to a single
 * on/off flip at the halfway mark: the link still tracks the child's lifecycle, it
 * just never animates along its length.
 */
export function linkDrawProgress(
  parent: LinkEndpointState,
  child: LinkEndpointState,
  instant = false,
): number {
  const p = Math.min(endpointDraw(parent), endpointDraw(child));
  if (!instant) return p;
  return p >= LINK_INSTANT_AT ? 1 : 0;
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
