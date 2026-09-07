// hudTransport.ts: the one seam between the HUD and whatever host is running it.
//
// The HUD now paints in two places: our own Tauri window, and the WARDEN section inside
// the boring.notch fork, which is a bare `WKWebView` with no Tauri runtime in it. The
// globes have to be IDENTICAL in both, and the only way that word means anything is if
// they are the same code, so nothing below this file is allowed to know which host it is
// in. `HudRoot` imports `invoke` and `listen` from here instead of from
// `@tauri-apps/api`, and that import swap is the entire difference between the two.
//
// The rule for anyone adding to the HUD: if you reach for a `@tauri-apps/api` symbol,
// add it HERE with an embedded implementation next to it. A direct import compiles fine
// and then throws in the section, where nobody is watching a console.

import { useEffect, useState } from 'react';

import { invoke as tauriInvoke } from '@tauri-apps/api/core';
import { listen as tauriListen } from '@tauri-apps/api/event';
import { getCurrentWindow as tauriGetCurrentWindow } from '@tauri-apps/api/window';

/** What a `listen` hands back: call it to stop listening. */
export type UnlistenFn = () => void;
/** The event shape the HUD destructures (`e.payload`). */
export type TransportEvent<T = unknown> = { event: string; payload: T };

type BridgeHandshake = { token: string };

declare global {
  interface Window {
    /**
     * Injected by the host BEFORE first paint (boring.notch does it with a
     * `WKUserScript` at document start). Preferred over the query string because a token
     * in a URL survives in history and can ride along in a `Referer`.
     */
    __WARDEN_BRIDGE__?: BridgeHandshake;
    __TAURI_INTERNALS__?: unknown;
    /** Installed by this module in the section; called by the host. See below. */
    __wardenSetVisible?: (visible: unknown) => void;
  }
}

/**
 * Are we inside our own Tauri window?
 *
 * `__TAURI_INTERNALS__` is injected by the runtime itself, so this is a fact about the
 * host rather than a build flag. One bundle therefore serves both, which is what keeps
 * the section from ever running a version of the globes that ours has not.
 */
export const isTauriHost = (): boolean =>
  typeof window !== 'undefined' && window.__TAURI_INTERNALS__ != null;

/** The bridge token, from the injected handshake or, failing that, `?token=`. */
function bridgeToken(): string {
  if (typeof window === 'undefined') return '';
  const injected = window.__WARDEN_BRIDGE__?.token;
  if (injected) return injected;
  try {
    return new URLSearchParams(window.location.search).get('token') ?? '';
  } catch {
    return '';
  }
}

/**
 * `invoke`, over the loopback bridge.
 *
 * Same-origin by construction: the page was served BY the bridge, so a relative URL is
 * already pointed at the right port and there is no port to hardcode or discover.
 *
 * Errors come back as `{ err }` with a 200 rather than an HTTP error status, matching
 * what Tauri does with a `Result::Err`: the call site's `.catch()` fires either way.
 */
async function bridgeInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const res = await fetch('/invoke', {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      authorization: `Bearer ${bridgeToken()}`,
    },
    body: JSON.stringify({ cmd, args: args ?? {} }),
  });
  if (!res.ok) throw new Error(`bridge ${cmd}: HTTP ${res.status}`);
  const body = (await res.json()) as { ok?: T; err?: string };
  if (body.err != null) throw new Error(body.err);
  return body.ok as T;
}

/**
 * One `EventSource` for the whole page, fanned out to every `listen` caller.
 *
 * Shared rather than one stream per listener because each stream costs a held connection
 * and, more to the point, each would replay its own opening `radar_state`: two listeners
 * would mean the HUD applying the same first state twice.
 *
 * `EventSource` reconnects on its own, so a WARDEN restart heals the section without it
 * knowing that happened. The bridge opens every stream with the current state, so that
 * reconnect repaints rather than leaving the last frame frozen on screen.
 */
type Handler = (e: TransportEvent) => void;
const handlers = new Map<string, Set<Handler>>();
let source: EventSource | null = null;

function ensureSource(): void {
  if (source || typeof window === 'undefined') return;
  // No EventSource, no stream. Real hosts all have one; this keeps the module importable
  // where it is absent (tests, SSR) instead of throwing out of a visibility change.
  if (typeof EventSource === 'undefined') return;
  // A `listen` registered while the host is hidden must not open a stream; the next
  // `setHostVisible(true)` opens it.
  if (!hostVisible) return;
  // The token goes in the query here and nowhere else: `EventSource` cannot set headers.
  source = new EventSource(`/events?token=${encodeURIComponent(bridgeToken())}`);
  source.onmessage = (m) => {
    let msg: TransportEvent;
    try {
      msg = JSON.parse(m.data) as TransportEvent;
    } catch {
      return;
    }
    const set = handlers.get(msg.event);
    if (!set) return;
    // Copy before iterating: a handler that unsubscribes itself would otherwise mutate
    // the set mid-iteration.
    for (const h of [...set]) h(msg);
  };
}

function bridgeListen<T>(
  event: string,
  cb: (e: TransportEvent<T>) => void,
): Promise<UnlistenFn> {
  ensureSource();
  const set = handlers.get(event) ?? new Set<Handler>();
  handlers.set(event, set);
  const h = cb as Handler;
  set.add(h);
  return Promise.resolve(() => {
    set.delete(h);
  });
}

/**
 * The window handle, or a stub that answers the one question the HUD asks of it.
 *
 * In the section there is no WARDEN window: boring.notch owns the panel, when it opens,
 * when it closes and whether it holds focus. `onFocusChanged` therefore never fires, and
 * that is CORRECT rather than a gap. The HUD's focus branch exists to decide whether its
 * own floating panel should start taking clicks; a section inside someone else's panel
 * has no such decision to make, and firing a synthetic focus event would make it dismiss
 * itself out from under its host.
 */
function stubWindow() {
  return {
    onFocusChanged: (_cb: (e: { payload: boolean }) => void): Promise<UnlistenFn> =>
      Promise.resolve(() => {}),
  };
}

/**
 * IS THE HOST ACTUALLY SHOWING US?
 *
 * In our own window this is always true: the window's existence IS the answer, and
 * `frameloopFor` already handles minimize and blur from Tauri's own signals.
 *
 * In the section it is the single most important number in this file. The notch panel
 * owns when the tab is on screen, and NOTHING about a hidden WKWebView tells the page
 * so: `document.hidden` stays false for a view merely taken out of the hierarchy, so
 * without this the scene would keep rendering. That is not a small waste. The HUD canvas
 * runs `frameloop='always'` whenever the panel is open, and in the section the panel is
 * ALWAYS open, so a WebGL scene would drive a 120Hz display forever behind a closed
 * notch, alongside a 750ms poll, for frames nobody can see.
 *
 * The host calls `window.__wardenSetVisible(false)` when the tab goes away or the notch
 * closes, and true when it comes back. Hidden also CLOSES the event stream, which is
 * what lets WARDEN skip serializing radar state altogether while nobody is attached.
 */
let hostVisible = true;
const visListeners = new Set<(v: boolean) => void>();

export function isHostVisible(): boolean {
  return hostVisible;
}

function setHostVisible(next: boolean): void {
  if (next === hostVisible) return;
  hostVisible = next;
  if (next) {
    ensureSource();
  } else {
    // Drop the stream rather than let it idle. An open EventSource still counts as a
    // subscriber on the Rust side, and one subscriber is the difference between WARDEN
    // serializing the whole radar state on every recompute and skipping it entirely.
    source?.close();
    source = null;
  }
  for (const cb of [...visListeners]) cb(next);
}

/** React hook form, for the canvas and the pull loop. */
export function useHostVisible(): boolean {
  const [v, setV] = useState(hostVisible);
  useEffect(() => {
    visListeners.add(setV);
    setV(hostVisible);
    return () => {
      visListeners.delete(setV);
    };
  }, []);
  return v;
}

if (typeof window !== 'undefined' && window.__TAURI_INTERNALS__ == null) {
  // Embedded only. Installed unconditionally so the host can call it before the React
  // tree exists, which it does: the tab can be hidden while the page is still booting.
  window.__wardenSetVisible = (v: unknown) => setHostVisible(v === true);
}

export const invoke = <T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> =>
  isTauriHost() ? (tauriInvoke as typeof bridgeInvoke)<T>(cmd, args) : bridgeInvoke<T>(cmd, args);

export const listen = <T = unknown>(
  event: string,
  cb: (e: TransportEvent<T>) => void,
): Promise<UnlistenFn> =>
  isTauriHost()
    ? (tauriListen as unknown as typeof bridgeListen)<T>(event, cb)
    : bridgeListen<T>(event, cb);

export const getCurrentWindow = () =>
  isTauriHost() ? tauriGetCurrentWindow() : (stubWindow() as ReturnType<typeof tauriGetCurrentWindow>);
