// observedToScene.ts: adapt a peer's redacted `ObservedState` into the scene model
// the constellation renders, so someone else's swarm can be shown as a real
// constellation instead of a list.
//
// The hard rule: this adapter INVENTS NOTHING. `ObservedAgent` deliberately has no
// name, title, path, or cost field, so every one of those comes out null or a
// visibly pseudonymous stand-in ("Agent 7f3a" against the salted hash the host
// actually sent). If a future field is added to the projection, it must be mapped
// here explicitly; anything unmapped stays null rather than being back-filled from
// a plausible guess. A viewer must never be able to mistake a derived placeholder
// for something the peer actually disclosed.
//
// Times are the other trap. A frame carries only RELATIVE times (`ageSecs`,
// `secsAgo`), on purpose, so an observer cannot correlate a peer's clock. We
// reconstruct absolute stamps against the FRAME's own `generatedAt`, never against
// the observer's local clock, so the reconstruction stays inside the peer's own
// timeline and a clock skew between the two machines changes nothing.

import type {
  RadarActivity,
  RadarAgent,
  RadarCurrentAction,
  RadarSceneModel,
  RadarStatus,
} from '@/viz/shared/types/radarTypes';
import type { ObservedAgent, ObservedState } from '@/viz/shared/types/observedTypes';

// `awaiting` crosses the wire because it is structure, not detail: an observer watching
// a shared board should see that a globe is blocked. The REASON does not cross, so a
// peer's globe strobes red and says "Waiting" without saying on what.
const STATUSES: ReadonlySet<string> = new Set([
  'working',
  'awaiting',
  'idle',
  'closed',
  'terminated',
]);

function asStatus(v: string): RadarStatus {
  return STATUSES.has(v) ? (v as RadarStatus) : 'idle';
}

/**
 * A short, obviously-derived display name. The id IS a salted hash, so its first
 * few characters are a stable per-frame handle with no recoverable meaning, which
 * is exactly what we want on a label: enough to tell two globes apart, not enough
 * to identify anything.
 */
export function observedLabel(agent: ObservedAgent): string {
  const short = agent.id.slice(0, 4) || '????';
  if (agent.depth > 0) return `Subagent ${short}`;
  return `Agent ${short}`;
}

/** frame time minus an offset in seconds, as an ISO string; '' if unparseable. */
function stampBefore(frameIso: string, secsAgo: number): string {
  const base = Date.parse(frameIso);
  if (!Number.isFinite(base)) return '';
  return new Date(base - Math.max(0, secsAgo) * 1000).toISOString();
}

function toAction(agent: ObservedAgent, frameIso: string): RadarCurrentAction | null {
  const a = agent.currentAction;
  if (!a) return null;
  return {
    kind: a.kind,
    tool: a.tool,
    // No target ever crosses the wire, so the label is the verb plus the tool and
    // nothing more. It must not read like a filename.
    label: a.tool,
    target: null,
    startedAt: stampBefore(frameIso, a.elapsedSecs),
    elapsedMs: Math.max(0, a.elapsedSecs) * 1000,
  };
}

function toActivity(agent: ObservedAgent, frameIso: string): RadarActivity[] {
  return agent.recentActivity.map((row) => ({
    ts: stampBefore(frameIso, row.secsAgo),
    kind: row.kind,
    // The projection sends a KIND and a time, never a label. Naming the kind is
    // the honest rendering of that; anything richer would be fabricated.
    label: row.kind,
    target: null,
  }));
}

function toAgent(agent: ObservedAgent, frameIso: string): RadarAgent {
  return {
    id: agent.id,
    harness: agent.harness,
    origin: null,
    parentId: agent.parentId,
    depth: agent.depth,
    label: observedLabel(agent),
    nickname: null,
    // "project A" is the host's own per-frame pseudonym, never a real folder.
    cwd: agent.project,
    // The pseudonym already IS the grouped identity: the host keys it on the repo,
    // so worktrees arrive pre-merged and an observer never learns a repo name.
    repo: null,
    role: agent.role,
    model: agent.model,
    title: null,
    surface: null,
    currentAction: toAction(agent, frameIso),
    team: agent.team
      ? {
          id: agent.team.id,
          // The team NAME is not on the wire; only its size and this member's role.
          name: 'Team',
          memberName: null,
          memberType: agent.team.memberType,
          memberCount: agent.team.memberCount,
          isLead: agent.team.isLead,
        }
      : null,
    status: asStatus(agent.status),
    contextTokens: agent.contextTokens,
    maxTokens: agent.maxTokens,
    fillPct: agent.fillPct,
    contextBreakdown: {
      usedTokens: agent.contextTokens,
      maxTokens: agent.maxTokens,
      fillPct: agent.fillPct,
      rows: agent.contextRows.map((r) => ({
        key: r.key,
        label: r.key,
        tokens: r.tokens,
        percent: Math.max(0, Math.min(1, r.percentX100 / 10000)),
        count: null,
      })),
    },
    // Composition is an exact API-anchored split locally. A peer sends no such
    // split, so it stays zeroed rather than being reconstructed from fill.
    composition: { exact: { cacheRead: 0, fresh: 0, cacheWrite: 0, output: 0 }, estimated: null },
    recentActivity: toActivity(agent, frameIso),
    childCount: agent.childCount,
    startedAt: stampBefore(frameIso, agent.ageSecs),
    // Cost is deliberately not on the wire. Null renders as an honest dash.
    estCostUsd: null,
  };
}

/**
 * Adapt a whole frame. A null/absent frame yields an empty forest, matching what
 * the local normalizer does, so a peer that has not sent a frame yet renders as
 * "nothing here" rather than as a crash.
 */
export function observedToScene(state: ObservedState | null): RadarSceneModel {
  if (!state) return { agents: [], generatedAt: '' };
  return {
    generatedAt: state.generatedAt,
    agents: state.agents.map((a) => toAgent(a, state.generatedAt)),
  };
}

export default observedToScene;
