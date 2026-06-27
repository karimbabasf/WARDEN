// Transition.tsx — swap one constellation for another by FOLDING.
//
// No starfield tricks, no camera move. Switching tabs simply collapses the current
// globes down to nothing, swaps the constellation while it's at zero size (so the
// change is unseen), then blooms the next globes back out. The whole constellation —
// orbs, tethers and labels — scales as one body, so it folds away and unfolds as a
// single coherent thing.
//
// One eased 0→1 progress drives a raised-cosine scale, |cos(pπ)|: full → 0 at the
// midpoint → full again. Its velocity is zero at both ends (it eases out of rest and
// back into rest, never a snap) and greatest at the invisible midpoint (so there's no
// dead pause at the singularity). The content swap is fired exactly at that midpoint.
// Pure scale ⇒ cheap, and reduced-motion just shortens it.

import { useRef, type ReactNode } from 'react';
import { useFrame } from '@react-three/fiber';
import * as THREE from 'three';

export type TransitionState = {
  active: boolean;
  /** Seconds elapsed in the current fold. */
  t: number;
  /** Has the mid-fold content swap fired this run? */
  fired: boolean;
  /** prefers-reduced-motion → a shorter fold. */
  reduced: boolean;
};

export function makeTransition(): TransitionState {
  return { active: false, t: 0, fired: false, reduced: false };
}

/** Begin a fold (collapse → swap → bloom). Mutates in place. */
export function beginTransition(ref: { current: TransitionState }): void {
  ref.current.active = true;
  ref.current.t = 0;
  ref.current.fired = false;
  ref.current.reduced =
    typeof window !== 'undefined' &&
    Boolean(window.matchMedia?.('(prefers-reduced-motion: reduce)').matches);
}

const DURATION = 0.9; // seconds — full fold-out + fold-in, eased
const REDUCED_DURATION = 0.4;
const FLOOR = 0.0001; // never scale to a degenerate 0 matrix

/** The raised-cosine fold curve: 1 → 0 (at p=0.5) → 1, eased at the ends. */
export function foldScale(p: number): number {
  const c = Math.min(1, Math.max(0, p));
  return Math.max(FLOOR, Math.abs(Math.cos(c * Math.PI)));
}

// Runs the fold down each frame into `scaleRef` (read live by the FoldGroups, no
// re-render per frame) and fires the swap at the midpoint / completion at the end.
export function TransitionDriver({
  stateRef,
  scaleRef,
  onMidpoint,
  onDone,
}: {
  stateRef: { current: TransitionState };
  scaleRef: { current: number };
  onMidpoint: () => void;
  onDone: () => void;
}) {
  useFrame((_, dtRaw) => {
    const s = stateRef.current;
    if (!s.active) {
      if (scaleRef.current !== 1) scaleRef.current = 1;
      return;
    }
    const dt = Math.min(dtRaw, 0.05);
    s.t += dt;
    const duration = s.reduced ? REDUCED_DURATION : DURATION;
    const p = Math.min(1, s.t / duration);
    scaleRef.current = foldScale(p);

    if (!s.fired && p >= 0.5) {
      s.fired = true;
      onMidpoint(); // swap the constellation while it's folded to nothing
    }
    if (p >= 1) {
      s.active = false;
      scaleRef.current = 1;
      onDone();
    }
  });
  return null;
}

// Scales its children uniformly from a live ref so the constellation folds as one.
// Wrap the constellation body (orbs + tethers + labels) in this.
export function FoldGroup({
  scaleRef,
  children,
}: {
  scaleRef: { current: number };
  children: ReactNode;
}) {
  const g = useRef<THREE.Group>(null!);
  useFrame(() => {
    if (g.current) g.current.scale.setScalar(scaleRef.current);
  });
  return <group ref={g}>{children}</group>;
}
