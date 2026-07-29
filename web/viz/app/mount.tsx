// mount.tsx: mounts the app EXACTLY ONCE into the pre-warmed hidden overlay
// window. Mount-once is the load-bearing perf rule: React + Three + the
// postprocessing composer initialise off the summon hot path, so by the time
// ⌘⌥⌃M fires the scene is already warm and only the bridge state changes
// thereafter.
//
// What mounts is decided by the license gate. `AppRoot` asks the backend once
// and renders either the activation screen or the war room; `main.ts` is
// unchanged and still just calls `mountWarRoom` and pipes events into the
// returned bridge.

import { useEffect, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { createBridge, type Bridge } from '@/viz/shared/state/bridge';
import { WarRoom } from '@/viz/views/war-room/WarRoom';
import { ActivationScreen } from '@/viz/views/activation/ActivationScreen';
import { normalizeStatus } from '@/viz/views/activation/activation';

let root: Root | null = null;
let bridge: Bridge | null = null;

type Phase = 'checking' | 'locked' | 'open';

/**
 * The gate's front door.
 *
 * `license_status` is one of the few commands a locked backend still answers, so
 * this is the one question that always gets a reply. Everything else the war
 * room calls is refused in Rust until a key verifies, which is why this router
 * is a convenience for the user and not the gate itself: deleting it would
 * change what is rendered, not what the backend will hand over.
 *
 * Fails CLOSED. A rejected or malformed reply leaves the app locked.
 */
function AppRoot({ bridge }: { bridge: Bridge }) {
  const [phase, setPhase] = useState<Phase>('checking');

  useEffect(() => {
    let alive = true;
    invoke('license_status')
      .then((s) => {
        if (!alive) return;
        setPhase(normalizeStatus(s).activated ? 'open' : 'locked');
      })
      .catch(() => {
        if (alive) setPhase('locked');
      });
    return () => {
      alive = false;
    };
  }, []);

  // A field-coloured hold rather than a spinner: the answer takes about a
  // millisecond, and a spinner that flashes for one frame is worse than nothing.
  if (phase === 'checking') return <div className="wd-activate-checking" />;
  if (phase === 'locked') return <ActivationScreen onActivated={() => setPhase('open')} />;
  return <WarRoom bridge={bridge} />;
}

/**
 * Mount the app into `rootId`. Idempotent: a second call returns the same bridge
 * without re-mounting React. The returned bridge is what `main.ts` (the single
 * Tauri event router) pipes events into via `bridge.ingest(name, payload)`.
 */
export function mountWarRoom(rootId: string): Bridge {
  if (bridge && root) return bridge;

  const el = document.getElementById(rootId);
  if (!el) {
    throw new Error(`mountWarRoom: #${rootId} not found`);
  }

  bridge = createBridge(listen);
  root = createRoot(el);
  root.render(<AppRoot bridge={bridge} />);
  return bridge;
}

/** Tear down (used by the dev-preview / HMR; the app mounts once for its life). */
export function unmountWarRoom(): void {
  root?.unmount();
  root = null;
  bridge = null;
}
