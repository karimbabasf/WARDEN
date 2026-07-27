// observedTypes.ts: the web-side mirror of `ObservedState` (Rust ->
// `src-tauri/src/observe/projection.rs`), the ONE shape that ever crosses the wire
// toward a remote observer.
//
// This is a SEPARATE type tree from `radarTypes.ts` on purpose. `ObservedAgent` is not
// a narrowed `RadarAgent`: it shares no fields by accident, has no name/label/title/path/
// prompt/cost field to omit, and nothing here should ever be widened to smuggle one in.
// `normalizeObservedState` exists for the same honest-viz reason `normalizeRadarState`
// does: a peer's frame crossed a network boundary (a different Warden build, a stale
// cache, a malformed payload), so a garbage frame must degrade to an empty forest rather
// than throw and blank the observer's view.

export type ObservedContextRow = {
  key: string;
  tokens: number;
  /** Hundredths of a percent (Rust `percent_x100`): 4260 = 42.60%. */
  percentX100: number;
};

export type ObservedAction = {
  kind: string; // read | write | search | run | tool
  tool: string; // an allowlisted builtin name, or "tool" for anything else
  elapsedSecs: number;
};

export type ObservedActivity = {
  kind: string;
  /** Seconds before the frame's `generatedAt`, never an absolute time. */
  secsAgo: number;
};

export type ObservedTeam = {
  /** Salted hash of the real team id. */
  id: string;
  memberCount: number;
  isLead: boolean;
  memberType: string | null;
};

/**
 * One agent as a remote observer sees it. Every field is numeric, drawn from a closed
 * vocabulary, or a salted hash: there is no field capable of carrying free text. Do not
 * add a name/label/title/path/prompt/cost field here, that is precisely what the
 * projection on the Rust side exists to keep off the wire.
 */
export type ObservedAgent = {
  id: string;
  parentId: string | null;
  harness: string; // 'claude_code' | 'codex' | 'unknown', same vocabulary as RadarAgent
  depth: number;
  status: string; // working | idle | closed | terminated
  /** Stable per-frame pseudonym for the project folder, e.g. "project A". Never a path. */
  project: string | null;
  role: string | null; // allowlisted agent type only
  model: string | null;
  contextTokens: number;
  maxTokens: number;
  fillPct: number;
  contextRows: ObservedContextRow[];
  childCount: number;
  /** Seconds since this agent started, relative to the frame. Never an absolute time. */
  ageSecs: number;
  currentAction: ObservedAction | null;
  recentActivity: ObservedActivity[];
  team: ObservedTeam | null;
};

export type ObservedState = {
  generatedAt: string;
  agents: ObservedAgent[];
  /** True when the host's 64-agent cap dropped agents from this frame. */
  truncated: boolean;
};

// coercion helpers (deliberately re-derived here, not imported from radarTypes: this
// file stands alone as the observer-side contract with zero coupling to the local
// radar's normalizer).
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

function nonNeg(v: unknown): number {
  const n = num(v);
  return n < 0 ? 0 : Math.round(n);
}

function normalizeContextRow(v: any): ObservedContextRow | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  const key = str(v.key);
  if (!key) return null;
  return {
    key,
    tokens: nonNeg(v.tokens),
    percentX100: nonNeg(v.percentX100 ?? v.percent_x100),
  };
}

function normalizeAction(v: any): ObservedAction | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  const kind = str(v.kind);
  const tool = str(v.tool);
  if (!kind || !tool) return null;
  return { kind, tool, elapsedSecs: nonNeg(v.elapsedSecs ?? v.elapsed_secs) };
}

function normalizeActivity(v: any): ObservedActivity {
  return { kind: str(v?.kind, 'message'), secsAgo: nonNeg(v?.secsAgo ?? v?.secs_ago) };
}

function normalizeTeam(v: any): ObservedTeam | null {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  const id = str(v.id);
  if (!id) return null;
  return {
    id,
    memberCount: nonNeg(v.memberCount ?? v.member_count),
    isLead: v.isLead === true || v.is_lead === true,
    memberType: strOrNull(v.memberType ?? v.member_type),
  };
}

function normalizeObservedAgent(a: any): ObservedAgent {
  return {
    id: str(a?.id),
    parentId: strOrNull(a?.parentId ?? a?.parent_id),
    harness: str(a?.harness, 'unknown'),
    depth: nonNeg(a?.depth),
    status: str(a?.status, 'idle'),
    project: strOrNull(a?.project),
    role: strOrNull(a?.role),
    model: strOrNull(a?.model),
    contextTokens: nonNeg(a?.contextTokens ?? a?.context_tokens),
    maxTokens: nonNeg(a?.maxTokens ?? a?.max_tokens),
    fillPct: clamp01(num(a?.fillPct ?? a?.fill_pct)),
    contextRows: arr(a?.contextRows ?? a?.context_rows)
      .map(normalizeContextRow)
      .filter((r): r is ObservedContextRow => r !== null),
    childCount: nonNeg(a?.childCount ?? a?.child_count),
    ageSecs: nonNeg(a?.ageSecs ?? a?.age_secs),
    currentAction: normalizeAction(a?.currentAction ?? a?.current_action),
    recentActivity: arr(a?.recentActivity ?? a?.recent_activity).map(normalizeActivity),
    team: normalizeTeam(a?.team),
  };
}

/**
 * Coerce a raw `ObservedState` frame (crossed the wire, or read back from a local
 * `invoke`) into a fully defaulted, safe shape. A malformed or partial frame yields an
 * empty forest rather than throwing: a peer running a newer/older Warden must never
 * blank the observer's whole view over one bad frame.
 */
export function normalizeObservedState(payload: any): ObservedState {
  return {
    generatedAt: str(payload?.generatedAt ?? payload?.generated_at),
    agents: arr(payload?.agents).map(normalizeObservedAgent),
    truncated: payload?.truncated === true,
  };
}
