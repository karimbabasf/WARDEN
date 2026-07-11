// emphasis.ts: the pure legend-filter model. A harness chip lights a single
// `EmphasisFilter`; matching globes stay full-strength while the rest dim (the dim
// channel is wired in the constellation). Pure so it is unit-tested in node without
// a render (see emphasis.test.ts).

export type EmphasisFilter =
  | { kind: 'harness'; harness: string }
  | null;

export interface EmphasisNode {
  harness?: string | null;
}

export function matchesFilter(node: EmphasisNode, filter: EmphasisFilter): boolean {
  if (filter === null) return true;
  if (filter.kind === 'harness') {
    if (node.harness === null || node.harness === undefined) return false;
    return node.harness === filter.harness;
  }
  return false;
}

export function targetDim(node: EmphasisNode, filter: EmphasisFilter): number {
  if (filter === null) return 0;
  return matchesFilter(node, filter) ? 0 : 1;
}
