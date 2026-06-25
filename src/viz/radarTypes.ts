// radarTypes.ts — the web-side mirror of the frozen `radar_state` contract
// (Rust → web, camelCase). RADAR's data model: a live forest of agents/subagents.
//
// `RadarSceneModel` is what the whole radar constellation consumes — layout,
// palette, lifecycle and the detail panel all read these fields and nothing else,
// so the viz can never invent a signal the backend did not emit. Every globe maps
// to a real agent; `normalizeRadarState` is the single honest seam that coerces a
// raw (possibly drifted) payload into a fully defaulted, safe model — schema drift
// must never throw or drop the forest.

/** Liveness of one agent. Mirrors Rust `AgentStatus`. */
export type RadarStatus = 'working' | 'idle' | 'closed' | 'terminated';

/** A single recent event tailing in an agent's context. */
export type RadarActivity = {
  ts: string;
  kind: 'tool' | 'message' | 'thinking' | string;
  label: string;
};

/** API-anchored token split (always present, from the transcript). */
export type RadarExactComposition = {
  cacheRead: number;
  fresh: number;
  output: number;
};

/** Locally-estimated semantic buckets (labeled "est." in the UI), or null. */
export type RadarEstComposition = {
  preamble: number;
  conversation: number;
  toolOutput: number;
  thinking: number;
};

export type RadarComposition = {
  exact: RadarExactComposition;
  /** null when there is no first turn to anchor the estimate against. */
  estimated: RadarEstComposition | null;
};

/** One node in the forest — a root agent or a (sub-)subagent. */
export type RadarAgent = {
  id: string;
  harness: string; // 'claude_code' | 'codex' | string
  origin: string | null; // 'Codex Desktop' | 'codex_vscode' | 'claude-desktop' | null
  parentId: string | null; // null = root
  depth: number; // 0 = root, 1 = subagent, …
  label: string;
  nickname: string | null;
  cwd: string | null; // project-folder basename (root only), for the "folder · model" subtitle
  role: string | null;
  model: string | null;
  status: RadarStatus;
  contextTokens: number; // exact live occupancy
  maxTokens: number; // model window (0 if unknown)
  fillPct: number; // contextTokens/maxTokens clamped [0,1]; 0 if maxTokens==0
  composition: RadarComposition;
  recentActivity: RadarActivity[];
  childCount: number;
  startedAt: string;
  estCostUsd: number | null;
};

/** The normalized forest the constellation renders. */
export type RadarSceneModel = {
  agents: RadarAgent[];
  generatedAt: string;
};

/** Collapse a full model id to a glanceable family name (claude-opus-4-8 → opus). */
export function shortModel(m: string | null): string | null {
  if (!m) return null;
  const s = m.toLowerCase();
  if (s.includes('opus')) return 'opus';
  if (s.includes('sonnet')) return 'sonnet';
  if (s.includes('haiku')) return 'haiku';
  if (s.includes('gpt-5')) return 'gpt-5';
  return m;
}

/**
 * The secondary "folder · model" identity line shown under an agent's name. Only
 * meaningful when the folder ADDS information beyond the label — i.e. the label is
 * the agent's task (Claude roots), not the folder itself (Codex). Returns null when
 * there's nothing useful to add, so the UI renders no empty subtitle.
 */
export function radarSubtitle(agent: Pick<RadarAgent, 'label' | 'cwd' | 'model'>): string | null {
  const folder = agent.cwd && agent.cwd !== (agent.label || '') ? agent.cwd : null;
  if (!folder) return null;
  const m = shortModel(agent.model);
  return m ? `${folder} · ${m}` : folder;
}

// ── coercion helpers (shared shape with bridge.ts; kept local so radarTypes has
// no import cycle and can be unit-tested in isolation) ─────────────────────────
function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0;
}

function str(v: unknown, fallback = ''): string {
  return typeof v === 'string' && v.length > 0 ? v : fallback;
}

function strOrNull(v: unknown): string | null {
  return typeof v === 'string' && v.length > 0 ? v : null;
}

function arr(v: unknown): any[] {
  return Array.isArray(v) ? v : [];
}

function clamp01(v: number): number {
  if (!Number.isFinite(v)) return 0;
  if (v < 0) return 0;
  if (v > 1) return 1;
  return v;
}

const STATUSES: ReadonlySet<string> = new Set(['working', 'idle', 'closed', 'terminated']);
function status(v: unknown): RadarStatus {
  return typeof v === 'string' && STATUSES.has(v) ? (v as RadarStatus) : 'idle';
}

function normalizeExact(v: any): RadarExactComposition {
  return {
    cacheRead: num(v?.cacheRead ?? v?.cache_read),
    fresh: num(v?.fresh),
    output: num(v?.output),
  };
}

function normalizeEstimated(v: any): RadarEstComposition | null {
  // honest-viz: only a well-formed object becomes an estimated lens; anything
  // else collapses to null so the panel shows no fabricated "est." bar.
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  return {
    preamble: num(v.preamble),
    conversation: num(v.conversation),
    toolOutput: num(v.toolOutput ?? v.tool_output),
    thinking: num(v.thinking),
  };
}

function normalizeActivity(v: any): RadarActivity {
  return {
    ts: str(v?.ts),
    kind: str(v?.kind, 'message'),
    label: str(v?.label),
  };
}

function normalizeAgent(a: any): RadarAgent {
  const comp = a?.composition;
  return {
    id: str(a?.id),
    harness: str(a?.harness, 'unknown'),
    origin: strOrNull(a?.origin),
    parentId: strOrNull(a?.parentId ?? a?.parent_id),
    depth: Math.max(0, Math.round(num(a?.depth))),
    label: str(a?.label),
    nickname: strOrNull(a?.nickname),
    cwd: strOrNull(a?.cwd),
    role: strOrNull(a?.role),
    model: strOrNull(a?.model),
    status: status(a?.status),
    contextTokens: num(a?.contextTokens ?? a?.context_tokens),
    maxTokens: num(a?.maxTokens ?? a?.max_tokens),
    fillPct: clamp01(num(a?.fillPct ?? a?.fill_pct)),
    composition: {
      exact: normalizeExact(comp?.exact),
      estimated: normalizeEstimated(comp?.estimated),
    },
    recentActivity: arr(a?.recentActivity ?? a?.recent_activity).map(normalizeActivity),
    childCount: Math.max(0, Math.round(num(a?.childCount ?? a?.child_count))),
    startedAt: str(a?.startedAt ?? a?.started_at),
    estCostUsd: typeof a?.estCostUsd === 'number' && Number.isFinite(a.estCostUsd)
      ? a.estCostUsd
      : typeof a?.est_cost_usd === 'number' && Number.isFinite(a.est_cost_usd)
        ? a.est_cost_usd
        : null,
  };
}

/**
 * Coerce a raw `radar_state` payload into a fully defaulted `RadarSceneModel`.
 * Tolerant of missing optionals, out-of-range numbers, and malformed sub-objects:
 * a garbage payload yields an empty forest rather than throwing.
 */
export function normalizeRadarState(payload: any): RadarSceneModel {
  return {
    generatedAt: str(payload?.generatedAt ?? payload?.generated_at),
    agents: arr(payload?.agents).map(normalizeAgent),
  };
}
