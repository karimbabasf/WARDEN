// bridge.ts: the honest seam between Tauri events and the radar scene.
//
// A PURE reducer: Tauri events in, an immutable `SceneState` out. Zero React/Three
// coupling, so it is trivially unit-testable (see bridge.test.ts) and the scene can
// never invent a signal the backend did not emit. The live agent forest is
// normalized through one honest seam (`normalizeRadarState`) so schema drift never
// throws or invents a globe. `harness` is always snake_case ("claude_code" |
// "codex" | "unknown").

import { normalizeRadarState, type RadarSceneModel } from '@/viz/shared/types/radarTypes';

export type SceneState = {
  /** Live RADAR forest (open agents/subagents), from the `radar_state` event. */
  radarScene?: RadarSceneModel;
  /** True while the daemon has the window summoned. The native `.show()` does not
   *  drive the webview Page Visibility API, so this explicit signal (routed from the
   *  `warden_hotkey` Tauri event by main.ts) is the authoritative wake signal for the
   *  R3F render loop. */
  summoned?: boolean;
  /** True while the window is MINIMIZED, the one and only animation gate. Blur and
   *  moving to another display do NOT set this: the render keeps running off-focus
   *  and only halts (CPU saver) when the window is actually minimized. */
  minimized?: boolean;
};

function emptyState(): SceneState {
  return { minimized: false };
}

/**
 * Fold one Tauri event into the current scene state, returning a NEW immutable
 * snapshot (or the same reference when the event is irrelevant/malformed, since
 * schema drift must never throw or drop the scene).
 */
export function reduce(state: SceneState, name: string, payload: any): SceneState {
  switch (name) {
    case 'radar_scene_ready':
      // The live agent forest (backend `radar_state`), normalized through the one
      // honest seam so schema drift can never throw or invent a globe.
      return { ...state, radarScene: normalizeRadarState(payload) };

    case 'warden_hotkey':
      // Daemon summoned the window. The native `.show()` does not drive the webview
      // Page Visibility API, so this is the authoritative wake signal (resumes the
      // render loop). It also clears the minimize pause.
      return state.summoned && !state.minimized
        ? state
        : { ...state, summoned: true, minimized: false };

    case 'warden_dismiss':
      // Window was hidden, so let the render loop pause.
      return state.summoned ? { ...state, summoned: false } : state;

    case 'warden_minimized':
      return state.minimized ? state : { ...state, minimized: true };

    case 'warden_restored':
      return state.minimized ? { ...state, minimized: false } : state;

    default:
      // Ingest progress, schema drift, and anything else non scene-driving: ignore
      // without mutating.
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
 * bridge can self-wire in the app and stay trivially testable in node). The caller
 * routes events into `ingest` from `main.ts` (the single router).
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
      // The persistent radar forest plus the window state (summon, minimize) survive.
      const { radarScene, summoned, minimized } = state;
      state = { ...emptyState(), radarScene, summoned, minimized };
      emit();
    },
  };
}
