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
import { hudAgents } from './hudSort';

/** How long the open spring needs before the panel counts as arrived. */
const SETTLE_MS = 420;

export function HudRoot() {
  const [agents, setAgents] = useState<RadarAgent[]>([]);
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
      if (alive) setAgents(hudAgents(normalizeRadarState(payload)));
    };
    invoke('get_radar_state').then(apply).catch(() => {});
    const un = listen('radar_state', (e) => apply(e.payload));
    return () => {
      alive = false;
      un.then((f) => f()).catch(() => {});
    };
  }, []);

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

  // Picking a globe is the HUD's one control: it hands the agent to the war room and
  // gets out of the way. Closing first would race the window it is about to raise.
  const onPick = useCallback(
    (agent: RadarAgent) => {
      invoke('hud_focus_agent', { agentId: agent.id }).catch(() => {});
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
        agents={agents}
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
