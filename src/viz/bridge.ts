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
};

/** Hard ceiling on rendered candidate nodes; overflow becomes `clustered`. */
export const NODE_CAP = 24;
/** Keep only the most recent pulses so the scene never unbounded-grows. */
const PULSE_CAP = 48;

function emptyState(): SceneState {
  return { phase: 'idle', candidates: [], verdicts: {}, pulses: [], usage: {}, clustered: 0 };
}

function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0;
}

function str(v: unknown, fallback = 'unknown'): string {
  return typeof v === 'string' && v.length > 0 ? v : fallback;
}

// Map raw token volume onto a 0..1 intensity. Log-scaled because token counts
// span orders of magnitude; small streams still register, huge ones saturate.
function tokenIntensity(tokens: number): number {
  if (tokens <= 0) return 0.12;
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
      return { ...state, phase, pulses };
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
      const pulse: Pulse = { stage, intensity, id: ++pulseSeq };
      const pulses = [...state.pulses, pulse].slice(-PULSE_CAP);
      return { ...state, usage: { ...state.usage, [stage]: usage }, pulses };
    }

    case 'diagnosis_ready': {
      // Capture the REAL diagnosis id (used as the reveal caption); the reveal's
      // findings derive from confirmed verdicts already in state.
      const id = typeof payload?.id === 'string' && payload.id.length > 0 ? payload.id : state.diagnosisId;
      return { ...state, phase: 'reveal', diagnosisId: id };
    }

    default:
      // ingest_progress, diagnosis_status, warden_hotkey, schema drift, … —
      // not scene-driving. Ignore without mutating.
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
      state = emptyState();
      emit();
    },
  };
}
