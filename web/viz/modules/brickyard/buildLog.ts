// buildLog.ts: the Brickyard build log as the web sees it. Mirrors Rust
// `radar::BuildStep` (radar/build_log.rs), which owns the mapping from tool calls to
// build actions. This side only normalizes, so a malformed row is dropped rather than
// inventing a brick.

export type BuildAction = 'place' | 'pull' | 'browse' | 'page' | 'spawn' | 'ask';

export type BuildStep = {
  seq: number;
  ts: string;
  action: BuildAction;
  tool: string;
  label: string;
  target: string | null;
  /** On a `pull`, the seq of the `place` it undoes. */
  undoes: number | null;
};

const ACTIONS: ReadonlySet<string> = new Set(['place', 'pull', 'browse', 'page', 'spawn', 'ask']);

export function normalizeBuildLog(raw: unknown): BuildStep[] {
  if (!Array.isArray(raw)) return [];
  const out: BuildStep[] = [];
  for (const r of raw) {
    if (!r || typeof r !== 'object') continue;
    const v = r as Record<string, unknown>;
    if (typeof v.seq !== 'number' || typeof v.action !== 'string' || !ACTIONS.has(v.action)) continue;
    out.push({
      seq: v.seq,
      ts: typeof v.ts === 'string' ? v.ts : '',
      action: v.action as BuildAction,
      tool: typeof v.tool === 'string' ? v.tool : '',
      label: typeof v.label === 'string' ? v.label : '',
      target: typeof v.target === 'string' && v.target.length > 0 ? v.target : null,
      undoes: typeof v.undoes === 'number' ? v.undoes : null,
    });
  }
  return out;
}
