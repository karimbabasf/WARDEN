// main.ts — the Tauri↔war-room router. The legacy terminal overlay is gone; the
// R3F war-room island IS the interface now. This module does exactly three jobs:
//   1. mount the island once into #war-room-root (pre-warmed on the hidden window),
//   2. pull boot state (profile, orb scene, cached diagnosis) into the bridge,
//   3. fan every Tauri event into the bridge and own the overlay lifecycle (summon
//      wake + minimize-pause). The overlay STAYS ON SCREEN and KEEPS ANIMATING when
//      it loses focus or moves to another display; the render loop pauses ONLY when
//      the window is minimized. It never auto-hides on blur or Esc.
// All rendering — HUD, ask bar, live pipeline, diagnosis drill-in — happens inside
// the island (src/viz/), driven purely by the bridge's SceneState. No DOM here.

import './style.css';
import { mountWarRoom } from './viz/mount';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

// ── lightweight diagnostics (the live overlay has no devtools by default) ──────
const diag = (m: string) => {
  invoke('diag', { msg: m }).catch(() => {});
};
window.addEventListener('error', (e) => diag(`JSERROR ${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener('unhandledrejection', (e) => diag(`REJECT ${String((e as PromiseRejectionEvent).reason)}`));
diag(`main.ts loaded hidden=${document.hidden} vis=${document.visibilityState} dpr=${window.devicePixelRatio}`);

const appWindow = getCurrentWindow();

// Mount the R3F war-room island ONCE; every Tauri event is routed into this bridge.
let bridge: ReturnType<typeof mountWarRoom>;
try {
  bridge = mountWarRoom('war-room-root');
  diag('mountWarRoom returned ok');
} catch (e) {
  diag(`mountWarRoom THREW ${String(e)}`);
  throw e;
}

// ── persistent orb scene refresh (debounced; FSEvents coalesces rapid writes) ──
let orbRefreshTimer: ReturnType<typeof window.setTimeout> | undefined;
let orbRefreshInFlight = false;
let orbRefreshQueued = false;

async function refreshOrbScene() {
  if (orbRefreshInFlight) {
    orbRefreshQueued = true;
    return;
  }
  orbRefreshInFlight = true;
  try {
    const orbScene = await invoke('get_orb_scene');
    bridge.ingest('orb_scene_ready', orbScene);
  } catch (e) {
    diag(`orb scene cold: ${String(e)}`);
  } finally {
    orbRefreshInFlight = false;
    if (orbRefreshQueued) {
      orbRefreshQueued = false;
      scheduleOrbSceneRefresh();
    }
  }
}

function scheduleOrbSceneRefresh(delayMs = 240) {
  if (orbRefreshTimer) window.clearTimeout(orbRefreshTimer);
  orbRefreshTimer = window.setTimeout(() => {
    orbRefreshTimer = undefined;
    refreshOrbScene();
  }, delayMs);
}

function schedulePostIngestOrbRefresh(phase: string, status?: string) {
  if (phase === 'live') scheduleOrbSceneRefresh(420);
  else if (status === 'complete') scheduleOrbSceneRefresh(80);
}

// ── boot: hydrate the bridge from persistent memory ───────────────────────────
async function boot() {
  try {
    const profile = await invoke('query_profile');
    bridge.ingest('profile_ready', profile);
  } catch (e) {
    diag(`profile cold: ${String(e)}`);
  }
  await refreshOrbScene();
  try {
    const cached = await invoke('get_diagnosis');
    if (cached) bridge.ingest('diagnosis_loaded', cached);
  } catch {
    // The cache is convenience only; a missing row must not break the overlay.
  }
}

// ── Tauri event fan-out → bridge (the single source of war-room state) ─────────
listen('ingest_progress', (e) => {
  const p = e.payload as { phase?: string; status?: string };
  bridge.ingest('ingest_progress', p);
  schedulePostIngestOrbRefresh(p.phase ?? '', p.status);
});

listen('diagnosis_status', (e) => bridge.ingest('diagnosis_status', e.payload));

listen('candidates_nominated', (e) => {
  bridge.ingest('candidates_nominated', e.payload);
  scheduleOrbSceneRefresh();
});

listen('finding_verdict', (e) => {
  bridge.ingest('finding_verdict', e.payload);
  scheduleOrbSceneRefresh();
});

listen('diagnosis_ready', (e) => {
  bridge.ingest('diagnosis_ready', e.payload);
  refreshOrbScene();
});

listen('fugu_delta', (e) => bridge.ingest('fugu_delta', e.payload));
listen('fugu_usage', (e) => bridge.ingest('fugu_usage', e.payload));

// RADAR: the backend's live agent-forest event. Forward verbatim — the bridge
// reducer normalizes it into SceneState.radarScene for the radar constellation.
listen('radar_state', (e) => bridge.ingest('radar_scene_ready', e.payload));

listen('warden_hotkey', () => {
  // The packaged app shows the pre-warmed HIDDEN window with a native call that
  // never fires the webview Page Visibility API, so this explicit summon signal —
  // not `visibilitychange` — is what wakes the render loop + fires the intro.
  bridge.ingest('warden_hotkey', {});
  diag(`hotkey received hidden=${document.hidden} vis=${document.visibilityState}`);
});

// The overlay STAYS ON SCREEN and KEEPS ANIMATING when it loses focus or you move to
// another display — animation is no longer tied to focus. The ONLY pause is minimize.
// Tauri has no dedicated minimize event, so we sample isMinimized() on every resize.
appWindow.onResized(async () => {
  try {
    bridge.ingest((await appWindow.isMinimized()) ? 'warden_minimized' : 'warden_restored', {});
  } catch {
    /* non-Tauri / dev surface: no-op */
  }
}).catch(() => {});

boot();
