// reducedMotion.ts: the R3F side of `prefers-reduced-motion`.
//
// style.css carries a global reduce rule, but it only reaches CSS. Anything animated
// inside a useFrame loop (globe spin rates, a link drawing along its length) has to
// ask for itself, and a scene-graph animation is exactly what that setting is about.
//
// Two entry points, deliberately: `prefersReducedMotion()` for a one-shot read inside
// an imperative frame callback, and `useReducedMotion()` for components that must
// RE-RENDER when the user flips the OS setting mid-session (the media query is live).

import { useSyncExternalStore } from 'react';

const QUERY = '(prefers-reduced-motion: reduce)';

/** One-shot read. Safe under SSR / jsdom, where matchMedia may be absent. */
export function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined') return false;
  return Boolean(window.matchMedia?.(QUERY).matches);
}

function subscribe(onChange: () => void): () => void {
  if (typeof window === 'undefined' || !window.matchMedia) return () => {};
  const mq = window.matchMedia(QUERY);
  // Safari < 14 only has the deprecated add/removeListener pair, so guard for the
  // modern one: a missing API degrades to "never updates" instead of throwing.
  if (typeof mq.addEventListener !== 'function') return () => {};
  mq.addEventListener('change', onChange);
  return () => mq.removeEventListener('change', onChange);
}

/**
 * Live subscription to the reduce setting. Returns the current value and re-renders
 * on change, so a component can pass one boolean down to every animated child rather
 * than having each of them register its own media-query listener.
 */
export function useReducedMotion(): boolean {
  return useSyncExternalStore(subscribe, prefersReducedMotion, () => false);
}
