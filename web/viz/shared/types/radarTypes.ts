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
  /**
   * Display path (`~`-folded) this row touched, or null for a row with no single
   * file target (a shell run, a message, thinking). Optional key so a fixture or
   * an older backend payload that never mentions it still satisfies the type;
   * `normalizeRadarState` always fills it in as a value or `null`.
   */
  target?: string | null;
};

/**
 * The single in-flight action: a tool call started and not yet returned. Mirrors
 * Rust `RadarAction`. `null` on the agent means genuinely idle between calls, not
 * "we could not tell" (that distinction is what makes it honest to render as the
 * panel hero).
 */
export type RadarCurrentAction = {
  kind: string; // read | write | search | run | tool, same vocabulary as RadarActivity.kind
  tool: string; // as the harness named it: "Edit", "Bash", "exec_command"
  label: string; // short human label, e.g. "Edit agent.rs"
  target: string | null; // ~-folded display path, when exactly one file is involved
  startedAt: string;
  elapsedMs: number;
};

/**
 * Membership in a named agent team. Mirrors Rust `RadarTeam`; Codex has no team
 * concept, so this stays null there.
 */
export type RadarTeam = {
  id: string;
  name: string;
  /** This agent's own name within the team, e.g. "BackendMap". */
  memberName: string | null;
  /** The agent type the team recorded for this member, e.g. "Explore". */
  memberType: string | null;
  memberCount: number;
  isLead: boolean;
};

/**
 * API-anchored token split (always present, from the transcript).
 *
 * `fresh` and `cacheWrite` are separate because they bill at different rates:
 * writing a token into the cache costs a premium over sending it fresh. They
 * used to be summed, which made the premium impossible to apply.
 */
export type RadarExactComposition = {
  cacheRead: number;
  fresh: number;
  cacheWrite: number;
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
  repo: string | null; // repo basename when `cwd` is a worktree/subdir of it, else null
  role: string | null;
  model: string | null;
  /**
   * The harness's own session title (Claude `custom-title`, or a Codex plan's H1),
   * distinct from `label` (radar-derived) and `nickname`. Optional key: a fixture
   * or older payload that omits it still satisfies the type; the normalizer always
   * fills it in as a string or `null`.
   */
  title?: string | null;
  /**
   * Where this session is being driven from, as the harness recorded it:
   * `cli` (a terminal), `claude-vscode` / `codex_vscode` (an IDE plugin),
   * `claude-desktop`, and so on. IDE-plugin sessions write into the SAME
   * transcript tree as terminal ones, so this is the only thing that tells
   * them apart, and people running an agent from inside their editor should
   * still see themselves tracked correctly. `null` when the harness did not
   * say. Optional key so an older payload still satisfies the type.
   */
  surface?: string | null;
  /** What this agent is doing right now; `null` when idle between tool calls. */
  currentAction?: RadarCurrentAction | null;
  /** Agent-team membership, when the harness groups agents into a named team. */
  team?: RadarTeam | null;
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
  if (s.includes('fable')) return 'fable';
  if (s.includes('gpt-5')) return 'gpt-5';
  return m;
}

/** Compact token magnitude for glance UIs: 172000 → "172k", 9_400_000 → "9.4M", 940 → "940". */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0';
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}k`;
  return String(Math.round(n));
}

/**
 * The secondary "folder · model" identity line shown under an agent's name. Only
 * meaningful when the folder ADDS information beyond the label — i.e. the label is
 * the agent's task (Claude roots), not the folder itself (Codex). Returns null when
 * there's nothing useful to add, so the UI renders no empty subtitle.
 */
export function radarSubtitle(
  agent: Pick<RadarAgent, 'label' | 'cwd' | 'model'> & { repo?: string | null },
): string | null {
  const folder = agent.cwd && agent.cwd !== (agent.label || '') ? agent.cwd : null;
  if (!folder) return null;
  // A worktree folder alone reads as an unrelated project, so lead with the repo it
  // belongs to. `repo` is only set when it differs from the folder.
  const place = agent.repo ? `${agent.repo}/${stripRepoPrefix(folder, agent.repo)}` : folder;
  const m = shortModel(agent.model);
  return m ? `${place} · ${m}` : place;
}

/// `git worktree add` folders are conventionally named after the repo
/// (`WARDEN-hotfix`), which would render as `WARDEN/WARDEN-hotfix`. Drop the repeat.
function stripRepoPrefix(folder: string, repo: string): string {
  for (const sep of ['-', '_', '.']) {
    const prefix = `${repo}${sep}`;
    if (folder.startsWith(prefix) && folder.length > prefix.length) return folder.slice(prefix.length);
  }
  return folder;
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
    cacheWrite: num(v?.cacheWrite ?? v?.cache_write),
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
    target: strOrNull(v?.target),
  };
}

/** A well-formed action always names both kind and tool; anything else collapses
 * to null so the hero never renders a half-populated action (honest-viz). */
function normalizeCurrentAction(v: any): RadarCurrentAction | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  const kind = str(v.kind);
  const tool = str(v.tool);
  if (!kind || !tool) return null;
  return {
    kind,
    tool,
    label: str(v.label),
    target: strOrNull(v.target),
    startedAt: str(v.startedAt ?? v.started_at),
    elapsedMs: Math.max(0, num(v.elapsedMs ?? v.elapsed_ms)),
  };
}

/** A well-formed team always has an id and a name; anything else collapses to null
 * rather than rendering a nameless team badge. */
function normalizeTeam(v: any): RadarTeam | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  const id = str(v.id);
  const name = str(v.name);
  if (!id || !name) return null;
  return {
    id,
    name,
    memberName: strOrNull(v.memberName ?? v.member_name),
    memberType: strOrNull(v.memberType ?? v.member_type),
    memberCount: Math.max(0, Math.round(num(v.memberCount ?? v.member_count))),
    isLead: v.isLead === true || v.is_lead === true,
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
    repo: strOrNull(a?.repo),
    role: strOrNull(a?.role),
    model: strOrNull(a?.model),
    title: strOrNull(a?.title),
    surface: strOrNull(a?.surface ?? a?.entrypoint),
    currentAction: normalizeCurrentAction(a?.currentAction ?? a?.current_action),
    team: normalizeTeam(a?.team),
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
