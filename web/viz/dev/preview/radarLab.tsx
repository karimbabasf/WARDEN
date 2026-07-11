// radarLab.tsx — standalone "radar studio" served by vite at /radar-lab.html.
//
// A mock-fed visual harness for the RADAR constellation (Task 23): it renders the
// REAL `RadarConstellation` (its own warm <Canvas>) + the REAL `RadarDetailPanel`
// against a hardcoded `RadarState` forest, with NO backend. This is the human/GPU
// pass surface — summon it in a browser to eyeball that every visual rule holds:
//
//   • depth-N hierarchy        — a Claude tree root → sub → sub-sub (planet → moon → sub-moon)
//   • Codex Desktop subagents  — root → 2 explorer moons with scientist nicknames
//   • FLAT VS Code Codex       — origin 'codex_vscode' renders solo, NO children (even
//                                with stray child data in the payload — honest-viz guard)
//   • unknown harness          — neutral slate globe + glyph, never a brand hue
//   • heat ramp                — fillPct low / mid / near-full → dim ember → white-hot
//   • status                   — working (bright + quick shimmer) vs idle (dim + slow breath)
//   • honest composition       — one agent has composition.estimated:null → panel shows "—"
//   • lifecycle + cross-fade   — exercised live (spawn/implode on toggle; tab fade lives in app)
//
// Reuse-first: it imports the live components and the real `normalizeRadarState`
// seam, so the mock is the SAME shape the backend emits — the harness can never
// drift from production data. Palette/layout/lifecycle all come from the app.

import { useCallback, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { RadarConstellation } from '@/viz/modules/radar/RadarConstellation';
import { RadarDetailPanel } from '@/viz/modules/radar/RadarDetailPanel';
import { normalizeRadarState } from '@/viz/shared/types/radarTypes';
import { isFlatAgent } from '@/viz/modules/radar/radarLayout';
import { radarHarness } from '@/viz/modules/radar/radarTheme';
import type { LayoutNode } from '@/viz/shared/types/orbTypes';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';
import '@/style.css';

// ── the mock forest, as a RAW contract payload (camelCase, exactly what Rust emits)
// run through `normalizeRadarState` so the harness data is production-shaped. Times
// are anchored relative to "now" so uptime/relative-time read sensibly on screen.
const now = Date.now();
const iso = (secsAgo: number) => new Date(now - secsAgo * 1000).toISOString();

const RAW_FOREST = {
  generatedAt: new Date(now).toISOString(),
  agents: [
    // ── 1) Claude tree, depth-2: root → subagent → sub-subagent ────────────────
    {
      id: 'cl-root',
      harness: 'claude_code',
      origin: 'claude-desktop',
      parentId: null,
      depth: 0,
      label: 'WARDEN',
      nickname: null,
      cwd: 'WARDEN',
      role: null,
      model: 'claude-opus-4-8',
      status: 'working',
      contextTokens: 188000, // near-full → blazing white-hot core
      maxTokens: 200000,
      fillPct: 0.94,
      composition: {
        exact: { cacheRead: 150000, fresh: 26000, output: 12000 },
        estimated: { preamble: 42000, conversation: 88000, toolOutput: 46000, thinking: 12000 },
      },
      recentActivity: [
        { ts: iso(2), kind: 'write', label: 'Edit RadarConstellation.tsx' },
        { ts: iso(8), kind: 'run', label: 'cargo test radar' },
        { ts: iso(14), kind: 'read', label: "sed -n '1,80p' assemble.rs" },
        { ts: iso(18), kind: 'thinking', label: 'Planning the cross-fade wiring' },
        { ts: iso(30), kind: 'search', label: 'rg "recentActivity" web/viz' },
        { ts: iso(48), kind: 'tool', label: 'browser_take_screenshot' },
        { ts: iso(55), kind: 'message', label: 'Wire the Habits↔Radar tab cross-fade' },
      ],
      childCount: 1,
      startedAt: iso(3600),
      estCostUsd: 4.12,
    },
    {
      id: 'cl-sub',
      harness: 'claude_code',
      origin: 'claude-desktop',
      parentId: 'cl-root',
      depth: 1,
      label: 'Explore · map radar frontend',
      nickname: null,
      role: 'Explore',
      model: 'claude-opus-4-8',
      status: 'idle', // idle → dim + slow breathing
      contextTokens: 64000, // mid fill
      maxTokens: 200000,
      fillPct: 0.32,
      composition: {
        exact: { cacheRead: 48000, fresh: 11000, output: 5000 },
        estimated: { preamble: 30000, conversation: 20000, toolOutput: 12000, thinking: 2000 },
      },
      recentActivity: [
        { ts: iso(120), kind: 'tool', label: 'Read src/viz/radarLayout.ts' },
        { ts: iso(140), kind: 'tool', label: 'Grep "layoutRadarScene"' },
      ],
      childCount: 1,
      startedAt: iso(900),
      estCostUsd: 0.68,
    },
    {
      id: 'cl-subsub',
      harness: 'claude_code',
      origin: 'claude-desktop',
      parentId: 'cl-sub',
      depth: 2,
      label: 'Explore · trace lifecycle reconciler',
      nickname: null,
      role: 'Explore',
      model: 'claude-haiku-4-5',
      status: 'working',
      contextTokens: 9000, // low fill → dim ember
      maxTokens: 200000,
      fillPct: 0.045,
      composition: {
        exact: { cacheRead: 4000, fresh: 4000, output: 1000 },
        // honest-viz: no first turn to anchor → estimated is NULL (panel shows "—")
        estimated: null,
      },
      recentActivity: [{ ts: iso(6), kind: 'tool', label: 'Read src/viz/radarLifecycle.ts' }],
      childCount: 0,
      startedAt: iso(240),
      estCostUsd: 0.04,
    },

    // ── 2) Codex Desktop tree: root → 2 explorer moons (scientist nicknames) ────
    {
      id: 'cx-root',
      harness: 'codex',
      origin: 'Codex Desktop',
      parentId: null,
      depth: 0,
      label: 'payments-api',
      nickname: null,
      cwd: 'payments-api',
      role: null,
      model: 'gpt-5-codex',
      status: 'working',
      contextTokens: 147000, // mid-high fill
      maxTokens: 258400,
      fillPct: 0.57,
      composition: {
        exact: { cacheRead: 110000, fresh: 28000, output: 9000 },
        estimated: { preamble: 38000, conversation: 70000, toolOutput: 32000, thinking: 7000 },
      },
      recentActivity: [
        { ts: iso(4), kind: 'tool', label: 'apply_patch packages/api/src/router.ts' },
        { ts: iso(30), kind: 'message', label: 'Refactor the auth middleware' },
      ],
      childCount: 2,
      startedAt: iso(2400),
      estCostUsd: 2.05,
    },
    {
      id: 'cx-dirac',
      harness: 'codex',
      origin: 'Codex Desktop',
      parentId: 'cx-root',
      depth: 1,
      label: 'explorer',
      nickname: 'Dirac',
      role: 'explorer',
      model: 'gpt-5-codex',
      status: 'working',
      contextTokens: 52000,
      maxTokens: 258400,
      fillPct: 0.2,
      composition: {
        exact: { cacheRead: 40000, fresh: 9000, output: 3000 },
        estimated: { preamble: 24000, conversation: 18000, toolOutput: 9000, thinking: 1000 },
      },
      recentActivity: [{ ts: iso(9), kind: 'tool', label: 'rg "verifyToken" packages/api' }],
      childCount: 0,
      startedAt: iso(600),
      estCostUsd: 0.41,
    },
    {
      id: 'cx-hilbert',
      harness: 'codex',
      origin: 'Codex Desktop',
      parentId: 'cx-root',
      depth: 1,
      label: 'explorer',
      nickname: 'Hilbert',
      role: 'explorer',
      model: 'gpt-5-codex',
      status: 'idle',
      contextTokens: 21000, // low-mid fill
      maxTokens: 258400,
      fillPct: 0.081,
      composition: {
        exact: { cacheRead: 15000, fresh: 5000, output: 1000 },
        estimated: { preamble: 14000, conversation: 5000, toolOutput: 1500, thinking: 500 },
      },
      recentActivity: [{ ts: iso(200), kind: 'message', label: 'Summarize the session schema' }],
      childCount: 0,
      startedAt: iso(540),
      estCostUsd: 0.17,
    },

    // ── 3) FLAT VS Code Codex agent — origin codex_vscode, NO children ──────────
    // Includes a STRAY child below to prove the honest-viz flat guard drops it.
    {
      id: 'cx-vscode',
      harness: 'codex',
      origin: 'codex_vscode',
      parentId: null,
      depth: 0,
      label: 'webapp',
      nickname: null,
      cwd: 'webapp',
      role: null,
      model: 'gpt-5-codex',
      status: 'working',
      contextTokens: 96000,
      maxTokens: 258400,
      fillPct: 0.37,
      composition: {
        exact: { cacheRead: 72000, fresh: 18000, output: 6000 },
        estimated: { preamble: 30000, conversation: 44000, toolOutput: 18000, thinking: 4000 },
      },
      recentActivity: [{ ts: iso(12), kind: 'tool', label: 'apply_patch src/app/page.tsx' }],
      childCount: 0,
      startedAt: iso(800),
      estCostUsd: 1.1,
    },
    {
      // a malformed/drifted child claiming the FLAT VS Code agent as parent — the
      // layout + roster guard must refuse to orbit it under cx-vscode (it surfaces
      // as its own solo root instead). Proves the guard end-to-end in the harness.
      id: 'cx-vscode-stray',
      harness: 'codex',
      origin: 'codex_vscode',
      parentId: 'cx-vscode',
      depth: 1,
      label: 'stray (should NOT be a moon of webapp)',
      nickname: null,
      cwd: 'webapp',
      role: null,
      model: 'gpt-5-codex',
      status: 'idle',
      contextTokens: 4000,
      maxTokens: 258400,
      fillPct: 0.015,
      composition: { exact: { cacheRead: 2000, fresh: 1500, output: 500 }, estimated: null },
      recentActivity: [],
      childCount: 0,
      startedAt: iso(120),
      estCostUsd: null,
    },

    // ── 4) Unknown-harness agent — neutral slate globe + glyph, NO brand hue ────
    {
      id: 'unknown-1',
      harness: 'gemini', // not in RADAR_PALETTE → RADAR_NEUTRAL
      origin: null,
      parentId: null,
      depth: 0,
      label: 'side-experiment',
      nickname: null,
      role: null,
      model: null,
      status: 'idle',
      contextTokens: 30000,
      maxTokens: 0, // unknown window → fillPct 0 (gauge shows ∞)
      fillPct: 0,
      composition: { exact: { cacheRead: 0, fresh: 0, output: 0 }, estimated: null },
      recentActivity: [],
      childCount: 0,
      startedAt: iso(300),
      estCostUsd: null,
    },
  ],
};

const FOREST = normalizeRadarState(RAW_FOREST);

// A stable, sensible default selection: the Claude root (deepest, richest panel).
const DEFAULT_SELECTED = 'cl-root';

function RadarLab() {
  const [selectedId, setSelectedId] = useState<string | null>(DEFAULT_SELECTED);
  const [hoveredId, setHoveredId] = useState<string | null>(null);

  const onHover = useCallback((n: LayoutNode) => setHoveredId(n.id), []);
  const onLeave = useCallback((n: LayoutNode) => setHoveredId((cur) => (cur === n.id ? null : cur)), []);
  const onSelect = useCallback((n: LayoutNode) => setSelectedId(n.id), []);
  const onClear = useCallback(() => setSelectedId(null), []);
  const onJumpTo = useCallback((id: string) => setSelectedId(id), []);

  const selectedAgent = useMemo<RadarAgent | null>(
    () => FOREST.agents.find((a) => a.id === selectedId) ?? null,
    [selectedId],
  );
  // Real children only, mirroring WarRoom: a flat agent yields [] (no fabricated
  // roster) even though a stray child exists in the payload for cx-vscode.
  const children = useMemo<RadarAgent[]>(
    () =>
      selectedAgent && !isFlatAgent(selectedAgent)
        ? FOREST.agents.filter((a) => a.parentId === selectedAgent.id)
        : [],
    [selectedAgent],
  );

  return (
    <div className="viz-root viz-orb-map">
      <RadarConstellation
        model={FOREST}
        selectedId={selectedId}
        hoveredId={hoveredId}
        onHover={onHover}
        onLeave={onLeave}
        onSelect={onSelect}
        onClear={onClear}
      />

      {/* the real detail dock — same container WarRoom uses */}
      <div className={`wd-inspector wd-radar-dock ${selectedAgent ? 'is-open' : ''}`}>
        {selectedAgent ? (
          <RadarDetailPanel agent={selectedAgent} children={children} onJumpTo={onJumpTo} onClose={onClear} />
        ) : null}
      </div>

      {/* dev-only agent picker so every globe's panel can be inspected without
          having to click the exact mesh (the click-to-focus still works too). */}
      <div className="radar-lab-picker">
        <div className="radar-lab-mark">
          <span className="sig">WARDEN</span>
          <span className="ver">radar · mock studio</span>
        </div>
        <div className="radar-lab-list">
          {FOREST.agents.map((a) => {
            const t = radarHarness(a.harness);
            return (
              <button
                key={a.id}
                type="button"
                className={`radar-lab-chip${selectedId === a.id ? ' is-active' : ''}`}
                style={{ ['--harness' as string]: t.color }}
                onClick={() => setSelectedId(a.id)}
                title={`${t.label} · ${Math.round(a.fillPct * 100)}% · ${a.status}`}
              >
                <span className="radar-lab-glyph" aria-hidden>
                  {t.glyph}
                </span>
                <span className="radar-lab-name">{a.nickname || a.label || a.id}</span>
                <span className="radar-lab-fill">{Math.round(a.fillPct * 100)}%</span>
              </button>
            );
          })}
        </div>
        <div className="radar-lab-hint">drag orbit · scroll zoom · click a globe to dive</div>
      </div>
    </div>
  );
}

createRoot(document.getElementById('orb-root')!).render(<RadarLab />);
