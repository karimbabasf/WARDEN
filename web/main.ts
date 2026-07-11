// main.ts: the Tauri to radar router. The R3F radar island IS the interface. This
// module does three jobs: mount the island once into #war-room-root, pull the initial
// radar forest into the bridge, and fan every Tauri event into the bridge while owning
// the window lifecycle (summon wake + minimize-pause). The window STAYS ON SCREEN and
// KEEPS ANIMATING when it loses focus or moves to another display; the render loop
// pauses ONLY when the window is minimized. All rendering happens inside the island
// (web/viz/), driven purely by the bridge's SceneState.

import './style.css';
import { mountWarRoom } from '@/viz/app/mount';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';

// Lightweight diagnostics: the packaged window has no devtools by default, so JS
// errors are forwarded to the Rust log via the `diag` command.
const diag = (m: string) => {
  invoke('diag', { msg: m }).catch(() => {});
};
window.addEventListener('error', (e) => diag(`JSERROR ${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener('unhandledrejection', (e) => diag(`REJECT ${String((e as PromiseRejectionEvent).reason)}`));
diag(`main.ts loaded hidden=${document.hidden} vis=${document.visibilityState} dpr=${window.devicePixelRatio}`);

const appWindow = getCurrentWindow();

// Mount the R3F radar island ONCE; every Tauri event is routed into this bridge.
let bridge: ReturnType<typeof mountWarRoom>;
try {
  bridge = mountWarRoom('war-room-root');
  diag('mountWarRoom returned ok');
} catch (e) {
  diag(`mountWarRoom THREW ${String(e)}`);
  throw e;
}

// Boot: hydrate the radar forest immediately so a cold open is not empty before the
// first watcher push arrives.
async function boot() {
  try {
    const rs = await invoke('get_radar_state');
    bridge.ingest('radar_scene_ready', rs);
  } catch (e) {
    diag(`radar cold: ${String(e)}`);
  }
}

// RADAR: the backend's live agent-forest event. Forward verbatim; the bridge reducer
// normalizes it into SceneState.radarScene for the radar constellation.
listen('radar_state', (e) => bridge.ingest('radar_scene_ready', e.payload));

listen('warden_hotkey', () => {
  // The packaged app shows the window with a native call that never fires the webview
  // Page Visibility API, so this explicit summon signal (not `visibilitychange`) is
  // what wakes the render loop.
  bridge.ingest('warden_hotkey', {});
  diag(`hotkey received hidden=${document.hidden} vis=${document.visibilityState}`);
});

// The window STAYS ON SCREEN and KEEPS ANIMATING when it loses focus or you move to
// another display. The ONLY pause is minimize. Tauri has no dedicated minimize event,
// so we sample isMinimized() on every resize.
appWindow
  .onResized(async () => {
    try {
      bridge.ingest((await appWindow.isMinimized()) ? 'warden_minimized' : 'warden_restored', {});
    } catch {
      /* non-Tauri / dev surface: no-op */
    }
  })
  .catch(() => {});

boot();
