// HudRoot.tsx: the menu-bar HUD's own window.
//
// A second, borderless, transparent webview that hangs under WARDEN's tray icon and
// answers one question in half a second: does anything need me right now. It is not a
// small war room. It has no camera, no rails, no detail panel: a glance surface that
// grows a control is a window, and there is already a window.
//
// The window is oversized and mostly empty on purpose: the panel animates INSIDE it,
// so a size morph costs no native window resize (which cannot be springed and would
// tear at 120Hz). The empty part is the dismiss target.
//
// Lifecycle is owned by Rust, the tray click shows the window and emits `hud_summon`
// with the icon's own rect, but the CLOSE is owned here: the genie has to finish
// playing before the window may be hidden, so the driver calls back and only then does
// `hud_hide` fire.

import { useCallback, useEffect, useRef, useState } from 'react';
// NOT from '@tauri-apps/api'. This same file paints in two hosts now: our own Tauri
// window and the WARDEN section inside the boring.notch fork, which is a bare WKWebView
// with no Tauri runtime. `hudTransport` picks the transport at runtime, so everything
// below this line, the panel, the scene and the globes, is one body of code in both.
import { invoke, listen, getCurrentWindow, isTauriHost } from '@/viz/shared/state/hudTransport';
import { normalizeRadarState, type RadarAgent } from '@/viz/shared/types/radarTypes';
import { HudPanel, type HudNeck, type HudPhase } from './HudPanel';
import { hudTree, type HudNode } from './hudSort';

/** How long the open spring needs before the panel counts as arrived. */
const SETTLE_MS = 420;

/** Fallback pull while the panel is open, matching the war room's own. Push events stay
 *  the fast path; this only exists so a missed one cannot persist. */
export const HUD_VISIBLE_PULL_MS = 750;

/** How long a panel nobody asked for stays up before it leaves again.
 *
 *  An auto summon (an agent just stopped on the operator) is a NOTIFICATION, and the
 *  thing that separates a notification from an interruption is that it goes away by
 *  itself. Long enough to read a fleet of globes and decide, short enough that ignoring
 *  it costs nothing. Reaching for the panel cancels the clock for good — see `engage`. */
export const HUD_AUTO_LINGER_MS = 6000;

/** What Rust sends (or holds) to open the panel: where the tray icon is, and whether
 *  anybody asked. */
export type HudSummonPayload = { centreX?: number; width?: number; auto?: boolean };

/** Does this panel leave on its own clock?
 *
 *  Only an AUTO summon does, only while it is on screen, and only until the operator
 *  reaches for it. A tray click never does: the operator asked for that one and it stays
 *  until they click away, press Escape, or pick a globe. */
export function autoDismisses(auto: boolean, engaged: boolean, phase: HudPhase): boolean {
  if (!auto || engaged) return false;
  return phase === 'opening' || phase === 'open';
}

export function HudRoot() {
  const [nodes, setNodes] = useState<HudNode[]>([]);
  const [phase, setPhase] = useState<HudPhase>('closed');
  const [neck, setNeck] = useState<HudNeck>({ centreX: 0, width: 24 });
  const [windowW, setWindowW] = useState(() => window.innerWidth);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  // Was this summon asked for? A tray click behaves like any menu-bar panel; a summon
  // the operator did not make has to earn its place and then leave.
  const [auto, setAuto] = useState(false);
  const [engaged, setEngaged] = useState(false);
  const phaseRef = useRef<HudPhase>('closed');
  phaseRef.current = phase;

  const dismiss = useCallback(() => {
    setPhase((p) => (p === 'opening' || p === 'open' ? 'closing' : p));
  }, []);

  // CLAIM THE SUMMON WE WERE NOT ALIVE FOR. This webview does not boot until its window
  // is first shown, so the summon that showed it was emitted at a page that did not exist
  // yet. Rust holds the last one; ask for it on mount and open from the answer.
  //
  // No pending summon while the window is up means the window is STRANDED (a fresh
  // HudRoot draws no panel by definition), so hide it: "visible" has to keep meaning "the
  // panel is open" for the tray toggle and the auto-open to reason about it.
  useEffect(() => {
    invoke<{ centreX?: number; width?: number; auto?: boolean } | null>('hud_pending_summon')
      .then((p) => {
        if (!p) return invoke('hud_hide').catch(() => {});
        applySummon(p);
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // ── the live forest ────────────────────────────────────────────────────────
  useEffect(() => {
    let alive = true;
    const apply = (payload: unknown) => {
      if (alive) setNodes(hudTree(normalizeRadarState(payload)));
    };
    invoke('get_radar_state').then(apply).catch(() => {});
    const un = listen('radar_state', (e) => apply(e.payload));
    return () => {
      alive = false;
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  // The push feed above had NO fallback, and it was the only thing keeping this
  // window honest: one pull at mount, then `radar_state` forever. A single missed or
  // dropped emit therefore left the HUD showing a fleet that no longer exists, with
  // nothing to ever correct it, which is the "why am I seeing agents that are gone"
  // report. The war room has covered this since it was written (`RADAR_VISIBLE_PULL_MS`);
  // the HUD never did.
  //
  // Only while the panel is actually on screen. A hidden HUD showing a stale fleet is
  // nobody's problem, and polling it would be paying for a glance nobody is taking.
  useEffect(() => {
    if (phase === 'closed') return;
    let alive = true;
    const pull = () => {
      invoke('get_radar_state')
        .then((rs) => {
          if (alive) setNodes(hudTree(normalizeRadarState(rs)));
        })
        .catch(() => {});
    };
    pull(); // immediate, so an open never starts from a stale list
    const id = window.setInterval(pull, HUD_VISIBLE_PULL_MS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, [phase]);

  // One definition of "open the panel here", because there are two ways in now: the push
  // event, and the pull on mount that covers a webview which was not there for it.
  const applySummon = useCallback((p: HudSummonPayload | null | undefined) => {
    setWindowW(window.innerWidth);
    setNeck({
      centreX: Number.isFinite(p?.centreX) ? (p?.centreX as number) : window.innerWidth / 2,
      width: Math.max(16, Number.isFinite(p?.width) ? (p?.width as number) : 24),
    });
    setAuto(p?.auto === true);
    setEngaged(false);
    setPhase('opening');
  }, []);

  // ── summon / dismiss, driven by the tray ───────────────────────────────────
  useEffect(() => {
    const subs = [
      listen<HudSummonPayload>('hud_summon', (e) => applySummon(e.payload)),
      listen('hud_dismiss', () => dismiss()),
    ];
    return () => {
      for (const s of subs) s.then((f) => f()).catch(() => {});
    };
  }, [dismiss, applySummon]);

  useEffect(() => {
    if (phase !== 'opening') return;
    const t = window.setTimeout(() => setPhase((p) => (p === 'opening' ? 'open' : p)), SETTLE_MS);
    return () => window.clearTimeout(t);
  }, [phase]);

  // Clicking away is the ordinary way to close a menu-bar panel, and on macOS that
  // arrives as a blur rather than as a click we can see.
  useEffect(() => {
    const un = getCurrentWindow().onFocusChanged(({ payload: focused }) => {
      if (!focused && phaseRef.current !== 'closed') dismiss();
    });
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, [dismiss]);

  // Reaching for the panel is the deliberate act a tray click would have been, so from
  // here an auto summon behaves exactly like a clicked one: it stops leaving on its own
  // clock, and it takes the keyboard it declined on the way in. Focus is what a click
  // outside reports as a blur, which is the only "the user looked away" signal macOS
  // gives, and it also spends the activating click so the FIRST press lands on a globe.
  const engage = useCallback(() => {
    setEngaged((was) => {
      if (!was) invoke('hud_take_focus').catch(() => {});
      return true;
    });
  }, []);

  // A panel nobody asked for leaves by itself. Only while it is genuinely unengaged: the
  // clock is cancelled the moment the pointer arrives, and never runs for a tray click.
  useEffect(() => {
    if (!autoDismisses(auto, engaged, phase)) return;
    const t = window.setTimeout(dismiss, HUD_AUTO_LINGER_MS);
    return () => window.clearTimeout(t);
  }, [auto, engaged, phase, dismiss]);

  // Escape closes a panel we OWN. In the notch section we do not own one: the tab is
  // boring.notch's, it has its own Escape behaviour, and a dismiss here would blank the
  // section with no way back, since nothing in that host ever sends a second summon.
  useEffect(() => {
    if (!isTauriHost()) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') dismiss();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [dismiss]);

  const onClosed = useCallback(() => {
    setPhase('closed');
    setHoveredId(null);
    setAuto(false);
    setEngaged(false);
    invoke('hud_hide').catch(() => {});
  }, []);

  // Picking a globe is the HUD's one control: it takes you to the TERMINAL that session
  // is running in, the tab and window the agent actually lives in, resolved and raised
  // by the same read-and-raise path as the detail panel's "take me there". Only when
  // that cannot be done (an IDE-hosted session, a window that has since closed, an
  // Automation refusal) does it fall back to opening WARDEN on the agent, where the
  // detail panel states the reason and, for a refusal, the one remedy. The panel
  // genies away at once either way: the click is the decision, and whatever comes
  // forward arrives while the island funnels back into the tray. Only roots reach here
  // (see HudPanel).
  const onPick = useCallback(
    (agent: RadarAgent) => {
      const agentId = agent.id;
      const openWarden = () => invoke('hud_focus_agent', { agentId }).catch(() => {});
      invoke<{ ok?: boolean }>('focus_agent_terminal', { agentId })
        .then((out) => {
          if (!out?.ok) return openWarden();
        })
        .catch(openWarden);
      dismiss();
    },
    [dismiss],
  );

  return (
    <div
      className={`wd-hud-root is-${phase}`}
      onPointerOver={(e) => {
        // Bubbled from the panel: the pointer is ON it, not in the empty dismiss area
        // around it (which is this element itself).
        if (e.target !== e.currentTarget) engage();
      }}
      onPointerDown={(e) => {
        // Anywhere outside the panel is dismiss territory: the window is mostly empty
        // and a click that lands on nothing should not feel like a dead zone.
        if (e.target === e.currentTarget) dismiss();
        else engage();
      }}
    >
      <HudPanel
        nodes={nodes}
        phase={phase}
        neck={neck}
        windowW={windowW}
        hoveredId={hoveredId}
        onClosed={onClosed}
        onPick={onPick}
        onHover={setHoveredId}
      />
    </div>
  );
}
