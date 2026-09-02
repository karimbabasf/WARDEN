// hudLinger.ts: keep what just left the board mounted long enough to leave visibly.
//
// The HUD's cells are keyed by agent and each one animates for itself (see HudPanel),
// so a cell that vanished from `nodes` would simply unmount between two frames: the
// one motion that cannot be animated after the fact. This returns the live list, in
// order, followed by whatever left it in the last `ms`, flagged `leaving`. The grid is
// never laid out from this list: a leaving item holds no slot, so its neighbours close
// ranks at once while it fades where it stood.
//
// The departed set is derived DURING RENDER, not in an effect. An effect runs after the
// commit that already dropped the item, so for one frame the cell (and the whole globe
// behind it, geometry and materials included) would unmount, then mount again as a
// ghost: a blink, and a stall. Setting state mid-render makes React re-run this render
// before anything is committed, so the item goes from live to leaving in one commit.
//
// The frame loop stamps the exit on its own clock when it first sees the flag; this
// only decides how long the element stays mounted.

import { useEffect, useMemo, useState } from 'react';

export type Lingering<T> = { item: T; leaving: boolean };

type Ghost<T> = { item: T; at: number };

/** Air after `ms` before a ghost is unmounted: the exit runs on the frame clock, the
 *  prune on the wall clock, and the element must outlive the animation. */
const GRACE_MS = 40;

export function useLingering<T>(items: T[], idOf: (item: T) => string, ms: number): Lingering<T>[] {
  const [state, setState] = useState<{ items: T[]; ghosts: Ghost<T>[] }>({ items, ghosts: [] });

  if (state.items !== items) {
    const live = new Set(items.map(idOf));
    const departed = state.items.filter((t) => !live.has(idOf(t)));
    // A ghost that came back is live again and stops being one; a ghost that was
    // already leaving keeps its original clock.
    const kept = state.ghosts.filter((x) => !live.has(idOf(x.item)));
    const fresh = departed.filter((t) => !kept.some((x) => idOf(x.item) === idOf(t)));
    const unchanged = fresh.length === 0 && kept.length === state.ghosts.length;
    const at = performance.now();
    setState({
      items,
      ghosts: unchanged ? state.ghosts : [...kept, ...fresh.map((item) => ({ item, at }))],
    });
  }

  const ghosts = state.ghosts;
  useEffect(() => {
    if (ghosts.length === 0) return;
    const oldest = Math.min(...ghosts.map((g) => g.at));
    const t = window.setTimeout(
      () =>
        setState((s) => ({
          ...s,
          ghosts: s.ghosts.filter((x) => performance.now() - x.at < ms + GRACE_MS),
        })),
      Math.max(0, oldest + ms + GRACE_MS - performance.now()),
    );
    return () => window.clearTimeout(t);
  }, [ghosts, ms]);

  return useMemo(
    () => [
      ...items.map((item) => ({ item, leaving: false })),
      ...ghosts.map((g) => ({ item: g.item, leaving: true })),
    ],
    [items, ghosts],
  );
}
