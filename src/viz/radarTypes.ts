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

export type RadarContextRow = {
  key: string;
  label: string;
  tokens: number;
  percent: number;
  count: number | null;
  muted?: boolean;
};

export type RadarContextBreakdown = {
  usedTokens: number;
  maxTokens: number;
  fillPct: number;
  rows: RadarContextRow[];
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
  cwdPath: string | null; // home-abbreviated project path (`~/alpha/api`) — breaks basename collisions
  role: string | null;
  model: string | null;
  status: RadarStatus;
  contextTokens: number; // exact live occupancy
  maxTokens: number; // model window (0 if unknown)
  fillPct: number; // contextTokens/maxTokens clamped [0,1]; 0 if maxTokens==0
  contextBreakdown?: RadarContextBreakdown;
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
 * The secondary "location · model" identity line shown under an agent's name. The
 * location is the parent-aware short path (last two segments of `cwdPath`) when it
 * adds information beyond the label — this is what breaks the collision of two
 * roots whose folders share a basename (`…/alpha/api` vs `…/beta/api`). Legacy
 * payloads without `cwdPath` keep the old bare-folder behavior. The model renders
 * even when the location collapses — a name collision must never hide WHICH model
 * an agent runs. Returns null only when there is truly nothing to add.
 */
export function radarSubtitle(agent: Pick<RadarAgent, 'label' | 'cwd' | 'cwdPath' | 'model'>): string | null {
  const label = agent.label || '';
  const short = shortPath(agent.cwdPath);
  const legacy = agent.cwd && agent.cwd !== label ? agent.cwd : null;
  const loc = short && short !== label ? short : legacy;
  const m = shortModel(agent.model);
  if (loc && m) return `${loc} · ${m}`;
  if (loc) return loc;
  return m;
}

/**
 * A SHORT, glanceable name for an agent's in-scene billboard label (the text pinned
 * beside its globe on the hero radar). Prefers the most identifying human name —
 * role → nickname → label → a short model → id — and clamps it to `max` chars with an
 * ellipsis so a long task title never sprawls across the constellation. Pure so the
 * label logic is unit-tested without WebGL; the glyph is added by the renderer (colour
 * is ALWAYS paired with the glyph for color-blind a11y).
 */
export function radarGlobeLabel(
  agent: Pick<RadarAgent, 'label' | 'nickname' | 'role' | 'model' | 'id'>,
  max = 22,
): string {
  const raw =
    (agent.role && agent.role.trim()) ||
    (agent.nickname && agent.nickname.trim()) ||
    (agent.label && agent.label.trim()) ||
    shortModel(agent.model) ||
    agent.id ||
    'agent';
  if (raw.length <= max) return raw;
  // clamp on a word/segment boundary when one is near the limit, else hard-cut.
  const clipped = raw.slice(0, max - 1);
  const lastBreak = Math.max(clipped.lastIndexOf(' '), clipped.lastIndexOf('/'));
  const base = lastBreak >= max - 8 ? clipped.slice(0, lastBreak) : clipped;
  return `${base.trimEnd()}…`;
}

/** Last two segments of a home-abbreviated path (`~/alpha/api` → `alpha/api`). */
function shortPath(p: string | null | undefined): string | null {
  if (!p) return null;
  const segs = p.split('/').filter((s) => s && s !== '~');
  if (!segs.length) return null;
  return segs.slice(-2).join('/');
}

/**
 * Short host badge for a desktop-app session — the sub-label that distinguishes a
 * Claude/Codex *Desktop* workflow from the CLI while keeping the SAME harness colour
 * + glyph. The harness identity (e.g. "Claude") already carries the colour; this is
 * the suffix (e.g. "Claude · **Desktop**"). Returns `"Desktop"` for any `origin`
 * ending in "Desktop" ("Claude Desktop" / "Codex Desktop"), else `null` — so the VS
 * Code Codex card (`origin === 'codex_vscode'`) is unchanged and no badge is ever
 * fabricated for an origin-less session.
 */
export function radarOriginBadge(agent: Pick<RadarAgent, 'origin'>): string | null {
  const o = agent.origin?.trim();
  if (o && /desktop$/i.test(o)) return 'Desktop';
  return null;
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

function normalizeContextRow(v: any): RadarContextRow | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  const key = str(v.key);
  const label = str(v.label);
  if (!key || !label) return null;
  const rawCount = v.count;
  return {
    key,
    label,
    tokens: Math.max(0, num(v.tokens)),
    percent: clamp01(num(v.percent)),
    count: typeof rawCount === 'number' && Number.isFinite(rawCount) ? Math.max(0, Math.round(rawCount)) : null,
    muted: v.muted === true,
  };
}

function normalizeContextBreakdown(v: any): RadarContextBreakdown {
  return {
    usedTokens: Math.max(0, num(v?.usedTokens ?? v?.used_tokens)),
    maxTokens: Math.max(0, num(v?.maxTokens ?? v?.max_tokens)),
    fillPct: clamp01(num(v?.fillPct ?? v?.fill_pct)),
    rows: arr(v?.rows).map(normalizeContextRow).filter((row): row is RadarContextRow => row !== null),
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
    cwdPath: strOrNull(a?.cwdPath ?? a?.cwd_path),
    role: strOrNull(a?.role),
    model: strOrNull(a?.model),
    status: status(a?.status),
    contextTokens: num(a?.contextTokens ?? a?.context_tokens),
    maxTokens: num(a?.maxTokens ?? a?.max_tokens),
    fillPct: clamp01(num(a?.fillPct ?? a?.fill_pct)),
    contextBreakdown: normalizeContextBreakdown(a?.contextBreakdown ?? a?.context_breakdown),
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
