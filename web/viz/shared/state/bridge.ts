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
  /** True while the window is MINIMIZED, the one and only full STOP for the render
   *  loop. Blur does not set this: an unfocused window keeps rendering, just at a
   *  paced rate rather than display rate (see `focused`). */
  minimized?: boolean;
  /** True while the window has native focus, routed from Tauri's `onFocusChanged`.
   *  It is a RENDER RATE input only (see `frameloopFor`): the radar keeps ingesting
   *  and updating whether or not anyone is looking at it.
   *
   *  Native, not `window.blur`, for the same reason `summoned` exists: the packaged
   *  app drives its window with native calls that do not reliably reach the webview.
   *  `undefined` means no native signal has arrived yet (the browser dev harness),
   *  and the view falls back to the DOM focus listener. */
  focused?: boolean;
  /** The agent the menu-bar HUD asked the war room to select, from
   *  `warden_focus_agent`. Null until something asks. */
  focusAgentId?: string | null;
  /** Bumped on every focus request. The id alone is not enough: picking the SAME globe
   *  in the HUD twice must re-select it in the war room, and a state field that did not
   *  change would look to React like nothing happened. */
  focusNonce?: number;
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

    case 'warden_focus_agent': {
      // The HUD hands over an agent id and the war room dives onto that globe. A
      // malformed payload is ignored rather than clearing the current selection:
      // schema drift must never move the camera.
      const id = typeof payload?.agentId === 'string' && payload.agentId.length > 0 ? payload.agentId : null;
      if (!id) return state;
      return { ...state, focusAgentId: id, focusNonce: (state.focusNonce ?? 0) + 1 };
    }

    case 'warden_focused':
      return state.focused === true ? state : { ...state, focused: true };

    case 'warden_blurred':
      return state.focused === false ? state : { ...state, focused: false };

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
