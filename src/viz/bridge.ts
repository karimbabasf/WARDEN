// bridge.ts — the HONEST seam between Tauri events and the war-room scene.
//
// This module is a PURE reducer: Tauri events in, an immutable `SceneState`
// snapshot out. It has zero React/Three coupling so it is trivially unit-
// testable (see bridge.test.ts) and so the scene can never invent a signal the
// backend did not actually emit. Every visible thing in the war room traces to
// a real field here:
//   • node count          = real nominated-candidate count (clamped, overflow tracked)
//   • core flare / colour  = real `finding_verdict`
//   • stage size / glow    = real `fugu_usage` tokens (orchestration when present,
//                            degrading to plain token weight off-Fugu)
//   • travelling pulses    = real `fugu_delta` stream activity
//
// Event contract is LOCKED by Task 6 — these shapes match the Rust emitters
// exactly. `harness` is always snake_case ("claude_code" | "codex" | "unknown").

import type { OrbSceneModel } from './orbTypes';
import { normalizeRadarState, type RadarSceneModel } from './radarTypes';

export type ScenePhase = 'idle' | 'war' | 'reveal';

export type Candidate = {
  patternId: string;
  sessionId: string;
  harness: string;
  severityHint: number;
};

export type Verdict = {
  findingId: string;
  patternId: string;
  harness: string;
  verdict: 'confirmed' | 'refuted';
  severity: number;
};

export type StageUsage = { in: number; out: number; orchIn: number; orchOut: number };

export type HarnessRollup = { harness: string; sessions: number; events: number };

export type Profile = {
  sessions: number;
  events: number;
  findings: number;
  byHarness: HarnessRollup[];
};

/** Latest streamed reasoning snippet from the pipeline (honest: real fugu_delta). */
export type StreamSnippet = { stage: string; text: string };

export type Pulse = {
  /** Fugu stage this pulse belongs to (Diagnostician / Coach / Verifier / …). */
  stage: string;
  /** 0..1 visual weight — drives how bright/large a travelling pulse renders. */
  intensity: number;
  /** Monotonic id so the scene can spawn one travelling sprite per pulse. */
  id: number;
};

export type SceneState = {
  phase: ScenePhase;
  candidates: Candidate[];
  /** Keyed by finding id; confirmed persists & flares, refuted collapses. */
  verdicts: Record<string, Verdict>;
  /** Recent token-stream pulses (bounded ring — see PULSE_CAP). */
  pulses: Pulse[];
  /** Per-stage token usage; orchestration tokens present only on Fugu engines. */
  usage: Record<string, StageUsage>;
  /** Nodes beyond NODE_CAP that were folded into the cluster glyph. */
  clustered: number;
  /** Real diagnosis id from `diagnosis_ready` (drives the reveal caption). */
  diagnosisId?: string;
  /** Persistent C1 orb mind-map model. It is separate from live per-run signals. */
  orbScene?: OrbSceneModel;
  /** Live RADAR forest (open agents/subagents), from the `radar_state` event. Like
   *  `orbScene` it is persistent live state, not part of a single diagnosis run. */
  radarScene?: RadarSceneModel;
  /** True while the daemon has the overlay summoned. The packaged app shows the
   *  pre-warmed HIDDEN window with a native call that does NOT fire the webview
   *  Page Visibility API, so this explicit signal — routed from the `warden_hotkey`
   *  Tauri event by main.ts — is the authoritative wake signal for the R3F render
   *  loop + one-shot intro. */
  summoned?: boolean;
  /** True while the overlay window is MINIMIZED — the one and only animation gate.
   *  Tracked from the Tauri window resize → `isMinimized()` sample in main.ts. Blur
   *  and moving to another display do NOT set this: the render keeps running off-focus
   *  and only halts (CPU saver) when the window is actually minimized. */
  minimized?: boolean;
  /** Persistent memory profile (totals + per-harness rollup), from `query_profile`. */
  profile?: Profile;
  /** Human-readable status line for the chrome (replaces the old terminal status). */
  status?: string;
  /** Current Fugu stage (Diagnostician / Coach / Verifier) while a run streams. */
  stage?: string;
  /** Latest streamed reasoning snippet — drives the live pipeline rail. */
  stream?: StreamSnippet;
  /** True while a diagnosis run is in flight (ask → war room → diagnosis). */
  running?: boolean;
  /** Last/cached diagnosis object (opaque to the bridge; rendered by the chrome). */
  diagnosis?: unknown;
};

/** Hard ceiling on rendered candidate nodes; overflow becomes `clustered`. */
export const NODE_CAP = 24;
/** Keep only the most recent pulses so the scene never unbounded-grows. */
const PULSE_CAP = 48;

function emptyState(): SceneState {
  return { phase: 'idle', candidates: [], verdicts: {}, pulses: [], usage: {}, clustered: 0, minimized: false };
}

function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0;
}

function str(v: unknown, fallback = 'unknown'): string {
  return typeof v === 'string' && v.length > 0 ? v : fallback;
}

function arr(v: unknown): any[] {
  return Array.isArray(v) ? v : [];
}

function normalizeOrbScene(payload: any): OrbSceneModel {
  return {
    agents: arr(payload?.agents).map((a: any) => ({
      id: str(a?.id),
      harness: str(a?.harness),
      label: str(a?.label),
      glyph: str(a?.glyph, '●'),
      color: str(a?.color, '#76ff9d'),
      sessions: num(a?.sessions),
      eventCount: num(a?.event_count ?? a?.eventCount),
      totalLoad: num(a?.total_load ?? a?.totalLoad),
    })),
    issues: arr(payload?.issues).map((i: any) => ({
      id: str(i?.id),
      agentId: str(i?.agent_id ?? i?.agentId),
      harness: str(i?.harness),
      patternId: str(i?.pattern_id ?? i?.patternId),
      title: str(i?.title, 'Untitled issue'),
      count: num(i?.count),
      severity: num(i?.severity),
      rationale: str(i?.rationale, ''),
      estCostTokens: num(i?.est_cost_tokens ?? i?.estCostTokens),
      estCostMinutes: num(i?.est_cost_minutes ?? i?.estCostMinutes),
      frequency: num(i?.frequency),
      confidence: num(i?.confidence),
      sessionIds: arr(i?.session_ids ?? i?.sessionIds).map((s: any) => str(s, '')).filter(Boolean),
      evidence: arr(i?.evidence),
      findingId: typeof (i?.finding_id ?? i?.findingId) === 'string' ? (i.finding_id ?? i.findingId) : undefined,
      verifierVerdict: typeof (i?.verifier_verdict ?? i?.verifierVerdict) === 'string' ? (i.verifier_verdict ?? i.verifierVerdict) : undefined,
      status: typeof i?.status === 'string' ? i.status : undefined,
    })),
    links: arr(payload?.links).flatMap((l: any) => {
      const source = str(l?.source, '');
      const target = str(l?.target, '');
      return source && target ? [{ source, target, kind: 'agent_issue' as const }] : [];
    }),
    guidance: {
      doItems: arr(payload?.guidance?.do_items ?? payload?.guidance?.doItems).map((s: any) => str(s, '')).filter(Boolean),
      stopItems: arr(payload?.guidance?.stop_items ?? payload?.guidance?.stopItems).map((s: any) => str(s, '')).filter(Boolean),
    },
  };
}

function normalizeProfile(payload: any): Profile {
  const byHarness = arr(payload?.by_harness ?? payload?.byHarness)
    .map((r: any) => ({
      harness: str(r?.harness),
      sessions: num(r?.sessions),
      events: num(r?.events),
    }))
    .filter((r: HarnessRollup) => r.sessions > 0 || r.events > 0);
  return {
    sessions: num(payload?.session_count ?? payload?.sessions),
    events: num(payload?.event_count ?? payload?.events),
    findings: num(payload?.finding_count ?? payload?.findings),
    byHarness,
  };
}

// Map raw token volume onto a 0..1 intensity. Log-scaled because token counts
// span orders of magnitude; small streams still register, huge ones saturate.
function tokenIntensity(tokens: number): number {
  if (tokens <= 0) return 0;
  return Math.min(1, 0.12 + Math.log10(tokens + 10) / 5);
}

let pulseSeq = 0;

/**
 * Fold one Tauri event into the current scene state, returning a NEW immutable
 * snapshot (or the same reference when the event is irrelevant/malformed —
 * schema drift must never throw or drop the scene).
 */
export function reduce(state: SceneState, name: string, payload: any): SceneState {
  switch (name) {
    case 'orb_scene_ready':
      return { ...state, orbScene: normalizeOrbScene(payload) };

    case 'radar_scene_ready':
      // The live agent forest (backend `radar_state`). Normalized through the one
      // honest seam so schema drift can never throw or invent a globe.
      return { ...state, radarScene: normalizeRadarState(payload) };

    case 'candidates_nominated': {
      const raw = Array.isArray(payload?.candidates) ? payload.candidates : [];
      const mapped: Candidate[] = raw.map((c: any) => ({
        patternId: str(c?.pattern_id),
        sessionId: str(c?.session_id),
        harness: str(c?.harness),
        severityHint: num(c?.severity_hint),
      }));
      const candidates = mapped.slice(0, NODE_CAP);
      const clustered = Math.max(0, mapped.length - NODE_CAP);
      return { ...state, phase: 'war', candidates, clustered };
    }

    case 'finding_verdict': {
      const findingId = str(payload?.finding_id, '');
      if (!findingId) return state;
      const v: Verdict = {
        findingId,
        patternId: str(payload?.pattern_id),
        harness: str(payload?.harness),
        verdict: payload?.verdict === 'confirmed' ? 'confirmed' : 'refuted',
        severity: num(payload?.severity),
      };
      // A verdict is real evidence of war activity even if nodes arrived late.
      const phase: ScenePhase = state.phase === 'idle' ? 'war' : state.phase;
      return { ...state, phase, verdicts: { ...state.verdicts, [findingId]: v } };
    }

    case 'fugu_delta': {
      const stage = str(payload?.stage);
      const delta = typeof payload?.delta === 'string' ? payload.delta : '';
      // Pulse weight tracks how much text actually streamed this tick.
      const intensity = Math.min(1, 0.2 + delta.trim().length / 240);
      const pulse: Pulse = { stage, intensity, id: ++pulseSeq };
      const pulses = [...state.pulses, pulse].slice(-PULSE_CAP);
      const phase: ScenePhase = state.phase === 'idle' ? 'war' : state.phase;
      // Feed the live pipeline rail: keep a short rolling snippet per stage so the
      // chrome can show the model actually reasoning (real fugu_delta, never faked).
      const snippet = delta.replace(/\s+/g, ' ').trim();
      const sameStage = state.stream?.stage === stage;
      const text = snippet
        ? (sameStage ? `${state.stream?.text ?? ''} ${snippet}`.trim().slice(-200) : snippet.slice(0, 200))
        : state.stream?.text ?? '';
      const stream: StreamSnippet | undefined = snippet || sameStage ? { stage, text } : state.stream;
      return { ...state, phase, pulses, stage, stream, status: `${stage} · reasoning`, running: true };
    }

    case 'fugu_usage': {
      const stage = str(payload?.stage);
      const usage: StageUsage = {
        in: num(payload?.input_tokens),
        out: num(payload?.output_tokens),
        orchIn: num(payload?.orchestration_input_tokens),
        orchOut: num(payload?.orchestration_output_tokens),
      };
      // Emit a stage pulse sized by REAL token weight. Prefer orchestration
      // tokens (Fugu); when absent (off-Fugu engine) degrade to plain tokens —
      // never fabricate orchestration activity that did not happen.
      const orch = usage.orchIn + usage.orchOut;
      const plain = usage.in + usage.out;
      const intensity = tokenIntensity(orch > 0 ? orch : plain);
      const pulses =
        intensity > 0
          ? [...state.pulses, { stage, intensity, id: ++pulseSeq }].slice(-PULSE_CAP)
          : state.pulses;
      return { ...state, usage: { ...state.usage, [stage]: usage }, pulses };
    }

    case 'diagnosis_ready': {
      // Capture the REAL diagnosis id (used as the reveal caption); the reveal's
      // findings derive from confirmed verdicts already in state.
      const id = typeof payload?.id === 'string' && payload.id.length > 0 ? payload.id : state.diagnosisId;
      return { ...state, phase: 'reveal', diagnosisId: id, status: 'diagnosis ready', running: false };
    }

    case 'profile_ready':
      // Memory totals + per-harness rollup from `query_profile` / ingest completion.
      return { ...state, profile: normalizeProfile(payload) };

    case 'ingest_progress': {
      // Not node-driving, but it carries the live status line + (on completion)
      // refreshed totals for the HUD. Tolerates both the batch and live-tail shapes.
      const phase = str(payload?.phase, 'ingest');
      const phStatus = typeof payload?.status === 'string' ? payload.status : '';
      const status = phStatus ? `${phase} · ${phStatus}` : phase;
      const hasTotals =
        payload?.total_sessions != null || payload?.session_count != null || payload?.finding_count != null;
      let profile = state.profile;
      if (hasTotals) {
        const rollup = arr(payload?.by_harness)
          .map((r: any) => ({ harness: str(r?.harness), sessions: num(r?.sessions), events: num(r?.events) }))
          .filter((r: HarnessRollup) => r.sessions > 0 || r.events > 0);
        profile = {
          sessions: num(payload?.total_sessions ?? payload?.session_count ?? state.profile?.sessions),
          events: num(payload?.total_events ?? payload?.event_count ?? state.profile?.events),
          findings: num(payload?.finding_count ?? state.profile?.findings),
          byHarness: rollup.length ? rollup : state.profile?.byHarness ?? [],
        };
      }
      return { ...state, status, profile };
    }

    case 'diagnosis_status':
      return { ...state, status: `${str(payload?.phase)} · ${str(payload?.status)}` };

    case 'diagnosis_run':
      // Island-driven run lifecycle: { running, query? }. Clears the stale stream
      // on start so the live rail doesn't show last run's reasoning.
      return payload?.running
        ? { ...state, running: true, status: 'ingesting transcripts', stream: undefined }
        : { ...state, running: false };

    case 'diagnosis_run_failed':
      return { ...state, running: false, status: 'diagnosis failed' };

    case 'diagnosis_loaded':
      // Opaque last/cached diagnosis object for the summary panel. The bridge does
      // not interpret it; the chrome renders the headline + routes drill-in to orbs.
      return { ...state, diagnosis: payload ?? state.diagnosis };

    case 'warden_hotkey':
      // Daemon summoned the overlay. The native window .show() does not drive the
      // webview Page Visibility API, so this is the war-room's authoritative wake
      // signal (resumes the render loop + fires the one-shot intro). It also clears
      // the minimize pause: Dock/tray/hotkey restore can emit only this signal, not a
      // resize-derived `warden_restored`, so the visible overlay must never stay
      // logically minimized.
      return state.summoned && !state.minimized ? state : { ...state, summoned: true, minimized: false };

    case 'warden_dismiss':
      // Overlay lost focus / was hidden — let the render loop pause.
      return state.summoned ? { ...state, summoned: false } : state;

    case 'warden_minimized':
      return state.minimized ? state : { ...state, minimized: true };

    case 'warden_restored':
      return state.minimized ? { ...state, minimized: false } : state;

    default:
      // ingest_progress, diagnosis_status, schema drift, … — not scene-driving.
      // Ignore without mutating.
      return state;
  }
}

export type Bridge = {
  subscribe: (cb: (s: SceneState) => void) => () => void;
  ingest: (name: string, payload: any) => void;
  reset: () => void;
};

/**
 * Build a live bridge. `listen` is the Tauri event listener (passed in so the
 * bridge can self-wire in the app and stay trivially testable in node). The
 * caller is still responsible for routing events into `ingest` from `main.ts`
 * (the single router) — `listen` is accepted for parity with the locked
 * interface and reserved for future direct self-subscription.
 */
export function createBridge(
  _listen: typeof import('@tauri-apps/api/event').listen,
): Bridge {
  let state = emptyState();
  const subscribers = new Set<(s: SceneState) => void>();

  function emit() {
    for (const cb of subscribers) cb(state);
  }

  return {
    subscribe(cb) {
      subscribers.add(cb);
      cb(state); // push current snapshot immediately
      return () => {
        subscribers.delete(cb);
      };
    },
    ingest(name, payload) {
      const next = reduce(state, name, payload);
      if (next !== state) {
        state = next;
        emit();
      }
    },
    reset() {
      // Live run signals clear; the persistent memory (orb scene, radar forest,
      // profile) and the window state (summon, minimize) survive — none is part of
      // a single run.
      const { orbScene, radarScene, summoned, minimized, profile } = state;
      state = { ...emptyState(), orbScene, radarScene, summoned, minimized, profile };
      emit();
    },
  };
}
