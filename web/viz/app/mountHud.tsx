// mountHud.tsx: mounts the menu-bar HUD EXACTLY ONCE into its pre-warmed hidden
// window, for the same reason `mount.tsx` does it for the war room: React, Three and
// the WebGL context all initialise at launch, off the click path, so the first tray
// click paints an already-warm scene instead of standing up a renderer.
//
// The HUD keeps no bridge. It listens for `radar_state` itself and normalizes through
// the one honest seam; there is no window lifecycle to fold in, because the window's
// only two states are the ones the tray drives.

import { createRoot, type Root } from 'react-dom/client';
import { HudRoot } from '@/viz/views/hud/HudRoot';

let root: Root | null = null;

/** Mount into `rootId`. Idempotent: a second call is a no-op. */
export function mountHud(rootId: string): void {
  if (root) return;
  const el = document.getElementById(rootId);
  if (!el) throw new Error(`mountHud: #${rootId} not found`);
  root = createRoot(el);
  root.render(<HudRoot />);
}

/** Tear down (dev preview / HMR only; the window mounts once for its life). */
export function unmountHud(): void {
  root?.unmount();
  root = null;
}
