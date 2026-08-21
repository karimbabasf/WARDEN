// hudLab.tsx: standalone "menu-bar studio" served by vite at /hud-lab.html.
//
// The REAL `HudPanel` against a mock fleet, with no backend and no Tauri: the panel is
// deliberately Tauri-free (only `HudRoot` touches the window), so the whole animation, 
// spring expansion, stagger, genie collapse, is reachable in a browser.
//
// This is the surface the motion actually gets judged on. What to look for:
//   • the island grows OUT OF the fake tray icon, not out of the middle of nowhere
//   • the size follows the fleet: ?n=0 is a pill, ?n=7 is 4x2, ?n=23 caps and says +8
//   • the collapse FUNNELS into the icon (top narrows first) rather than scaling down
//   • the drop shadow follows the funnel instead of staying a rectangle
//   • awaiting globes and the header dot strobe on the SAME beat
//
// `?n=<count>` sets the fleet, `?slow=<factor>` stretches the motion for frame-by-frame
// review (the genie at 1x is 280ms, which is exactly too fast to fault by eye).
import { useCallback, useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { normalizeRadarState, type RadarStatus } from '@/viz/shared/types/radarTypes';
import { HudPanel, type HudPhase } from '@/viz/views/hud/HudPanel';
import { hudTree } from '@/viz/views/hud/hudSort';
import '@/hud.css';

const HARNESSES = ['claude_code', 'codex', 'claude_code', 'codex', 'unknown'];
const STATUSES: RadarStatus[] = ['working', 'awaiting', 'idle', 'working', 'idle'];
const FOLDERS = ['WARDEN', 'pakkr', 'switchboard', 'phosphor-lp', 'doxa', 'frontier', 'the-board'];

/** A deterministic fleet of `n` roots, shaped exactly like a real `radar_state`.
 *
 *  `kidSpread` is how many subagents the busiest root gets; the rest fan out under it
 *  so one board carries a bare session, a lightly-loaded one and a crowded one at
 *  once. That mix is the point: the panel's HEIGHT is what has to be judged here, and
 *  a fleet where every row is equally busy would never show a row growing. */
function mockFleet(n: number, kidSpread: number) {
  const roots = Array.from({ length: n }, (_, i) => ({
    id: `agent-${i}`,
    harness: HARNESSES[i % HARNESSES.length],
    depth: 0,
    parentId: null,
    label: `task ${i}`,
    cwd: FOLDERS[i % FOLDERS.length],
    model: i % 2 ? 'claude-opus-5' : 'gpt-5',
    status: STATUSES[i % STATUSES.length],
    awaitingReason: 'question',
    contextTokens: 40_000 + i * 9_000,
    maxTokens: 200_000,
    fillPct: Math.min(1, 0.1 + (i % 7) * 0.15),
    childCount: 0,
    startedAt: `2026-08-20T09:${String(10 + i).padStart(2, '0')}:00Z`,
    composition: { exact: { cacheRead: 1, fresh: 1, cacheWrite: 0, output: 1 }, estimated: null },
    recentActivity: [],
  }));
  const kids = roots.flatMap((root, i) => {
    const count = kidSpread === 0 ? 0 : (i * 3) % (kidSpread + 1);
    return Array.from({ length: count }, (_, k) => ({
      ...root,
      id: `${root.id}-sub-${k}`,
      // Every other strip is hung off a SUBAGENT rather than the root, so the
      // ancestry walk in `hudTree` is exercised and not just the depth-1 case.
      depth: k % 2 && k > 0 ? 2 : 1,
      parentId: k % 2 && k > 0 ? `${root.id}-sub-${k - 1}` : root.id,
      label: `sub ${k}`,
      status: STATUSES[(i + k) % STATUSES.length],
      childCount: 0,
    }));
  });
  return normalizeRadarState({
    generatedAt: '2026-08-20T12:00:00Z',
    agents: [...roots, ...kids],
  });
}

function query(key: string, fallback: number): number {
  const raw = new URLSearchParams(window.location.search).get(key);
  if (raw === null) return fallback;
  const v = Number(raw);
  // `>= 0`, not `> 0`: an empty fleet is the case worth checking (the island stays a
  // pill) and a `> 0` guard silently swapped it for the default.
  return Number.isFinite(v) && v >= 0 ? v : fallback;
}

function HudLab() {
  const [count, setCount] = useState(() => query('n', 7));
  const [kids, setKids] = useState(() => query('kids', 6));
  const [phase, setPhase] = useState<HudPhase>('closed');
  // `?hover=<n>` pins the nth cell hovered, so the hover PLATE (which lives under the
  // canvas, see HudPanel) is reachable in a static screenshot. The war room lab's
  // `?select=` exists for the same reason: a state you can only reach with a live
  // pointer is a state nobody checks.
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const nodes = useMemo(() => hudTree(mockFleet(count, kids)), [count, kids]);
  const pinnedHover = new URLSearchParams(window.location.search).get('hover');
  useEffect(() => {
    if (pinnedHover === null) return;
    const n = nodes[Number(pinnedHover)];
    if (n) setHoveredId(n.agent.id);
  }, [pinnedHover, nodes]);

  // The REAL window, faithfully: 600x540 (tauri.conf.json), placed centred under the
  // icon and clamped to the screen. Faking it with the full browser width would put the
  // neck somewhere production never puts it, and the neck is what the genie aims at.
  const WIN_W = 600;
  const WIN_H = 540;
  const MENUBAR_H = 24;
  const iconCentre = Math.max(120, window.innerWidth - 168);
  const winX = Math.min(Math.max(iconCentre - WIN_W / 2, 0), window.innerWidth - WIN_W);
  const open = useCallback(() => setPhase('opening'), []);

  useEffect(() => {
    if (phase !== 'opening') return;
    const t = window.setTimeout(() => setPhase((p) => (p === 'opening' ? 'open' : p)), 420);
    return () => window.clearTimeout(t);
  }, [phase]);

  // Open on load so a screenshot without a click still shows the panel.
  useEffect(() => {
    const t = window.setTimeout(open, 120);
    return () => window.clearTimeout(t);
  }, [open]);

  return (
    <>
      <div className="lab-menubar">
        <span className="lab-menubar-apple">WARDEN · menu-bar studio</span>
        <button
          type="button"
          className={`lab-tray${phase === 'closed' ? '' : ' is-live'}`}
          style={{ left: iconCentre - 11 }}
          onClick={() => (phase === 'closed' ? open() : setPhase('closing'))}
        >
          ◉
        </button>
      </div>

      <div className="lab-controls">
        <label>
          fleet
          <input
            type="range"
            min={0}
            max={23}
            value={count}
            onChange={(e) => setCount(Number(e.currentTarget.value))}
          />
          <b>{count}</b>
        </label>
        <label>
          subagents
          <input
            type="range"
            min={0}
            max={12}
            value={kids}
            onChange={(e) => setKids(Number(e.currentTarget.value))}
          />
          <b>{kids}</b>
        </label>
        <div className="lab-buttons">
          <button type="button" onClick={open}>open</button>
          <button type="button" onClick={() => setPhase('closing')}>genie</button>
        </div>
      </div>

      {/* The transparent HUD window. Outlined here only so its real bounds, and how
          much of it the island never uses, are visible while judging the motion. */}
      <div
        className="lab-window"
        style={{ left: winX, top: MENUBAR_H + 6, width: WIN_W, height: WIN_H }}
      >
        <div className="wd-hud-root">
          <HudPanel
            nodes={nodes}
            phase={phase}
            neck={{ centreX: iconCentre - winX, width: 22 }}
            windowW={WIN_W}
            hoveredId={hoveredId}
            onClosed={() => setPhase('closed')}
            onPick={(a) => setHoveredId(a.id)}
            onHover={setHoveredId}
          />
        </div>
      </div>
    </>
  );
}

const el = document.getElementById('hud-lab-root');
if (el) createRoot(el).render(<HudLab />);
