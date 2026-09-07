// hud.ts: entry point for the menu-bar HUD webview.
//
// Deliberately thin. The war room's `main.ts` is a router because it owns a window
// lifecycle (summon, minimize-pause, focus-paced render rate); the HUD has none of
// that. It is shown and hidden by the tray, it renders only while visible, and every
// event it cares about is subscribed inside the view that uses it.

import './hud.css';
import { invoke } from '@/viz/shared/state/hudTransport';
import { mountHud } from '@/viz/app/mountHud';

const diag = (m: string) => {
  invoke('diag', { msg: m }).catch(() => {});
};
window.addEventListener('error', (e) => diag(`HUD JSERROR ${e.message} @ ${e.filename}:${e.lineno}`));
window.addEventListener('unhandledrejection', (e) =>
  diag(`HUD REJECT ${String((e as PromiseRejectionEvent).reason)}`),
);

try {
  mountHud('hud-root');
} catch (e) {
  diag(`mountHud THREW ${String(e)}`);
  throw e;
}
