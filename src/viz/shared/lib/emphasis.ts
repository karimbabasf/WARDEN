export type EmphasisFilter =
  | { kind: 'severity'; bucket: 'low' | 'med' | 'high' | 'crit' }
  | { kind: 'harness'; harness: string }
  | null

export interface EmphasisNode {
  harness?: string | null
  severity?: number | null
}

export function severityBucket(severity: number): 'low' | 'med' | 'high' | 'crit' {
  if (severity <= 2) return 'low'
  if (severity === 3) return 'med'
  if (severity === 4) return 'high'
  return 'crit'
}

export function matchesFilter(node: EmphasisNode, filter: EmphasisFilter): boolean {
  if (filter === null) return true

  if (filter.kind === 'severity') {
    if (node.severity === null || node.severity === undefined) return false
    return severityBucket(node.severity) === filter.bucket
  }

  if (filter.kind === 'harness') {
    if (node.harness === null || node.harness === undefined) return false
    return node.harness === filter.harness
  }

  return false
}

export function targetDim(node: EmphasisNode, filter: EmphasisFilter): number {
  if (filter === null) return 0
  return matchesFilter(node, filter) ? 0 : 1
}
