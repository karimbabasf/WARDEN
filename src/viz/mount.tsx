// mount.tsx — mounts the R3F war-room island EXACTLY ONCE into the pre-warmed
// hidden overlay window. Mount-once is the load-bearing perf rule: React +
// Three + the postprocessing composer initialise off the summon hot path, so by
// the time ⌘⌥⌃M fires the scene is already warm and only the bridge state
// changes thereafter.

import { createRoot, type Root } from 'react-dom/client';
import { listen } from '@tauri-apps/api/event';
import { createBridge, type Bridge } from './bridge';
import { WarRoom } from './WarRoom';

let root: Root | null = null;
let bridge: Bridge | null = null;

/**
 * Mount the war room into `rootId`. Idempotent: a second call returns the same
 * bridge without re-mounting React. The returned bridge is what `main.ts` (the
 * single Tauri event router) pipes events into via `bridge.ingest(name, payload)`.
 */
export function mountWarRoom(rootId: string): Bridge {
  if (bridge && root) return bridge;

  const el = document.getElementById(rootId);
  if (!el) {
    throw new Error(`mountWarRoom: #${rootId} not found`);
  }

  bridge = createBridge(listen);
  root = createRoot(el);
  root.render(<WarRoom bridge={bridge} />);
  return bridge;
}

/** Tear down (used by the dev-preview / HMR; the app mounts once for its life). */
export function unmountWarRoom(): void {
  root?.unmount();
  root = null;
  bridge = null;
}
