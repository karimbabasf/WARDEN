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
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { normalizeRadarState, type RadarAgent } from '@/viz/shared/types/radarTypes';
import { HudPanel, type HudNeck, type HudPhase } from './HudPanel';
import { hudTree, type HudNode } from './hudSort';

/** How long the open spring needs before the panel counts as arrived. */
const SETTLE_MS = 420;

/** Fallback pull while the panel is open, matching the war room's own. Push events stay
 *  the fast path; this only exists so a missed one cannot persist. */
export const HUD_VISIBLE_PULL_MS = 750;

export function HudRoot() {
  const [nodes, setNodes] = useState<HudNode[]>([]);
  const [phase, setPhase] = useState<HudPhase>('closed');
  const [neck, setNeck] = useState<HudNeck>({ centreX: 0, width: 24 });
  const [windowW, setWindowW] = useState(() => window.innerWidth);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const phaseRef = useRef<HudPhase>('closed');
  phaseRef.current = phase;

  const dismiss = useCallback(() => {
    setPhase((p) => (p === 'opening' || p === 'open' ? 'closing' : p));
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

  // ── summon / dismiss, driven by the tray ───────────────────────────────────
  useEffect(() => {
    const subs = [
      listen<{ centreX: number; width: number }>('hud_summon', (e) => {
        const p = e.payload;
        setWindowW(window.innerWidth);
        setNeck({
          centreX: Number.isFinite(p?.centreX) ? p.centreX : window.innerWidth / 2,
          width: Math.max(16, Number.isFinite(p?.width) ? p.width : 24),
        });
        setPhase('opening');
      }),
      listen('hud_dismiss', () => dismiss()),
    ];
    return () => {
      for (const s of subs) s.then((f) => f()).catch(() => {});
    };
  }, [dismiss]);

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

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') dismiss();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [dismiss]);

  const onClosed = useCallback(() => {
    setPhase('closed');
    setHoveredId(null);
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
      onPointerDown={(e) => {
        // Anywhere outside the panel is dismiss territory: the window is mostly empty
        // and a click that lands on nothing should not feel like a dead zone.
        if (e.target === e.currentTarget) dismiss();
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
