import type { Vec3 } from '@/viz/shared/types/orbTypes';

/** User-dragged node positions, keyed by node id. */
export type PositionOverrides = Map<string, [number, number, number]>;

/** Stable empty map so memo deps don't churn when there are no overrides. */
export const NO_OVERRIDES: PositionOverrides = new Map();

/**
 * Returns a NEW layout whose overridden nodes carry the user-dragged position,
 * or the SAME object when nothing applies (so downstream memos keep identity and
 * don't needlessly re-bake). Single-node: only the dragged node moves; its links
 * and labels re-derive from the new position because they read node.position.
 */
export function applyLayoutOverrides<
  L extends { nodes: Array<{ id: string; position: Vec3 }> },
>(layout: L, overrides: PositionOverrides): L {
  if (overrides.size === 0) return layout;
  let changed = false;
  const nodes = layout.nodes.map((n) => {
    const o = overrides.get(n.id);
    if (!o) return n;
    changed = true;
    return { ...n, position: { x: o[0], y: o[1], z: o[2] } };
  });
  return changed ? ({ ...layout, nodes } as L) : layout;
}
