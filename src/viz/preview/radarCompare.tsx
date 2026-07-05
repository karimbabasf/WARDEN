// radarCompare.tsx — the "stop the line-up" A/B/C studio, served at /radar-compare.html.
//
// Renders the REAL `RadarConstellation` (same Canvas, camera, mesh + link path) but
// swaps the LAYOUT via the new `layoutFn` seam, so three geometries can be compared
// apples-to-apples against a synthetic fleet you can scale 6 → 40 agents:
//
//   • Baseline — the shipped tilted-disc layout (collapses to a line edge-on).
//   • Sphere   — Fibonacci volumetric, isotropic (no camera angle collapses it).
//   • Lens     — Fibonacci flattened to a thick lens (compromise; still has a bad ~90°).
//
// The fleet is biased toward the reported pain: one busy orchestrator spawning a swarm
// of subagents in ONE folder (plus a couple of smaller families at higher counts), so
// the child-placement — the real culprit at scale — is what gets stressed.

import { useMemo, useState, useCallback } from 'react';
import { createRoot } from 'react-dom/client';
import { RadarConstellation } from '../RadarConstellation';
import { normalizeRadarState, type RadarSceneModel } from '../radarTypes';
import { layoutRadarScene, type RadarLayout } from '../radarLayout';
import { layoutRadarDiscFlat, layoutRadarLens } from '../radarLayoutVariants';
import type { LayoutNode } from '../orbTypes';
import '../../style.css';

// ── synthetic fleet ────────────────────────────────────────────────────────────
const now = Date.now();
const iso = (secsAgo: number) => new Date(now - secsAgo * 1000).toISOString();
// deterministic 0..1 hash so a given (i, salt) always yields the same fleet.
const rnd = (i: number, salt: number): number => {
  const h = (Math.imul(i + 1, 2654435761) ^ Math.imul(salt + 1, 40503)) >>> 0;
  return (h % 100000) / 100000;
};

type Raw = Record<string, unknown>;

function agent(o: {
  id: string;
  parentId: string | null;
  depth: number;
  harness: string;
  folder: string;
  seed: number;
  role?: string | null;
}): Raw {
  const codex = o.harness === 'codex';
  const maxTokens = codex ? 258400 : 200000;
  // varied fill so globes carry real size variety; roots run hotter than subs.
  const fill = o.depth === 0 ? 0.45 + rnd(o.seed, 1) * 0.5 : 0.04 + rnd(o.seed, 2) * 0.45;
  const contextTokens = Math.round(maxTokens * fill);
  const working = rnd(o.seed, 3) > 0.28; // ~72% working, rest idle
  return {
    id: o.id,
    harness: o.harness,
    origin: codex ? 'Codex Desktop' : 'claude-desktop',
    parentId: o.parentId,
    depth: o.depth,
    label: o.depth === 0 ? o.folder : `${o.role ?? 'Explore'} · task ${o.seed}`,
    nickname: null,
    cwd: o.folder,
    role: o.role ?? null,
    model: codex ? 'gpt-5-codex' : o.depth === 0 ? 'claude-opus-4-8' : 'claude-haiku-4-5',
    status: working ? 'working' : 'idle',
    contextTokens,
    maxTokens,
    fillPct: fill,
    composition: {
      exact: {
        cacheRead: Math.round(contextTokens * 0.7),
        fresh: Math.round(contextTokens * 0.2),
        output: Math.round(contextTokens * 0.1),
      },
      estimated: null,
    },
    recentActivity: [{ ts: iso(2 + o.seed), kind: 'tool', label: 'Read src/viz/radarLayout.ts' }],
    childCount: 0,
    startedAt: iso(300 + o.seed * 37),
    estCostUsd: Number((rnd(o.seed, 4) * 4).toFixed(2)),
  };
}

const ROLES = ['Explore', 'Plan', 'general-purpose', 'code-review', 'Test', 'Debug'];

/**
 * Build a deterministic fleet of ~`n` agents. One dominant orchestrator owns the
 * lion's share of subagents (the "I spun up 10 agents" case); larger fleets add a
 * couple more, smaller families across a second folder, and a little depth-2 nesting.
 */
function makeFleet(n: number): RadarSceneModel {
  const agents: Raw[] = [];
  const folders = n > 18 ? ['WARDEN', 'trading-bot'] : ['WARDEN'];
  const orchCount = Math.max(1, Math.min(4, Math.round(n / 12)));
  const orchIds: string[] = [];
  for (let o = 0; o < orchCount; o++) {
    const folder = folders[o % folders.length];
    const harness = o % 3 === 2 ? 'codex' : 'claude_code';
    const id = `orch-${o}`;
    agents.push(agent({ id, parentId: null, depth: 0, harness, folder, seed: 100 + o }));
    orchIds.push(id);
  }
  let made = orchIds.length;
  let s = 0;
  // depth-1 subagents, weighted so orchestrator 0 (the "busy" one) gets ~45%.
  const depth1: { id: string; folder: string; harness: string }[] = [];
  while (made < n) {
    // weighted orchestrator pick — bias hard toward the first.
    const r = rnd(s, 7);
    const oi = r < 0.45 ? 0 : 1 + Math.floor(rnd(s, 8) * Math.max(1, orchCount - 1));
    const parentIdx = Math.min(oi, orchCount - 1);
    const parentId = orchIds[parentIdx];
    const folder = folders[parentIdx % folders.length];
    const harness = parentIdx % 3 === 2 ? 'codex' : 'claude_code';
    const id = `a${s}`;
    const role = ROLES[s % ROLES.length];
    agents.push(agent({ id, parentId, depth: 1, harness, folder, seed: s, role }));
    depth1.push({ id, folder, harness });
    made++;
    s++;
  }
  // sprinkle a little depth-2 nesting on the largest fleets so hierarchy shows.
  if (n >= 25 && depth1.length > 4) {
    const nest = Math.min(3, Math.floor(n / 12));
    for (let d = 0; d < nest; d++) {
      const host = depth1[(d * 3 + 1) % depth1.length];
      agents.push(
        agent({ id: `n${d}`, parentId: host.id, depth: 2, harness: host.harness, folder: host.folder, seed: 500 + d, role: 'Explore' }),
      );
    }
  }
  return normalizeRadarState({ generatedAt: new Date(now).toISOString(), agents });
}

// ── strategies ───────────────────────────────────────────────────────────────
type StratKey = 'flat' | 'sphere' | 'lens';
const STRATEGIES: Record<StratKey, { label: string; blurb: string; fn: (m: RadarSceneModel) => RadarLayout }> = {
  flat: { label: 'Flat', blurb: 'flat disc (the old failure) — lines up edge-on', fn: layoutRadarDiscFlat },
  sphere: { label: 'Sphere', blurb: 'SHIPPED — Fibonacci spheres + horizontal folders', fn: layoutRadarScene },
  lens: { label: 'Lens', blurb: 'flattened sphere — compromise, one bad angle', fn: layoutRadarLens },
};
const N_CHOICES = [6, 12, 25, 40];

function RadarCompare() {
  const [n, setN] = useState(12);
  const [strat, setStrat] = useState<StratKey>('sphere');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [hoveredId, setHoveredId] = useState<string | null>(null);

  const model = useMemo(() => makeFleet(n), [n]);
  const layoutFn = STRATEGIES[strat].fn;
  const nodeCount = useMemo(() => layoutFn(model).nodes.length, [layoutFn, model]);

  const onHover = useCallback((nd: LayoutNode) => setHoveredId(nd.id), []);
  const onLeave = useCallback((nd: LayoutNode) => setHoveredId((c) => (c === nd.id ? null : c)), []);
  const onSelect = useCallback((nd: LayoutNode) => setSelectedId(nd.id), []);
  const onClear = useCallback(() => setSelectedId(null), []);

  return (
    <div className="viz-root viz-orb-map">
      <RadarConstellation
        // remount on strategy/N change so the camera resets to the overview pose and
        // every comparison starts from the identical framing.
        key={`${strat}-${n}`}
        model={model}
        layoutFn={layoutFn}
        selectedId={selectedId}
        hoveredId={hoveredId}
        onHover={onHover}
        onLeave={onLeave}
        onSelect={onSelect}
        onClear={onClear}
      />

      <div className="rc-panel">
        <div className="rc-title">
          RADAR · layout compare
          <span className="rc-count">{nodeCount} globes</span>
        </div>

        <div className="rc-group-label">agents</div>
        <div className="rc-row">
          {N_CHOICES.map((c) => (
            <button key={c} type="button" className={`rc-btn${n === c ? ' is-on' : ''}`} onClick={() => setN(c)}>
              {c}
            </button>
          ))}
        </div>

        <div className="rc-group-label">geometry</div>
        <div className="rc-strats">
          {(Object.keys(STRATEGIES) as StratKey[]).map((k) => (
            <button key={k} type="button" className={`rc-strat${strat === k ? ' is-on' : ''}`} onClick={() => setStrat(k)}>
              <span className="rc-strat-name">{STRATEGIES[k].label}</span>
              <span className="rc-strat-blurb">{STRATEGIES[k].blurb}</span>
            </button>
          ))}
        </div>

        <div className="rc-hint">drag to orbit · scroll to zoom · orbit to ~90° to expose the edge-on collapse</div>
      </div>
    </div>
  );
}

const style = document.createElement('style');
style.textContent = `
  html, body { height: 100%; margin: 0; background: #020403; overflow: hidden; }
  #orb-root { position: fixed; inset: 0; }
  canvas { display: block; touch-action: none; }
  .rc-panel {
    position: fixed; top: 16px; left: 50%; transform: translateX(-50%); z-index: 10;
    display: flex; flex-direction: column; gap: 8px; width: min(560px, 92vw);
    padding: 12px 14px; border-radius: 12px;
    background: rgba(4,18,11,0.72); border: 1px solid rgba(118,255,157,0.16);
    backdrop-filter: blur(9px); -webkit-backdrop-filter: blur(9px);
    font-family: "SF Mono", Menlo, Consolas, monospace; color: #9fcdb2;
  }
  .rc-title { display: flex; align-items: baseline; gap: 10px; font-size: 12px; letter-spacing: 0.34em;
    text-transform: uppercase; color: #76ff9d; text-shadow: 0 0 16px rgba(118,255,157,0.5); }
  .rc-count { margin-left: auto; font-size: 10.5px; letter-spacing: 0.14em; color: #6f9d82; }
  .rc-group-label { font-size: 9.5px; letter-spacing: 0.22em; text-transform: uppercase; color: #4d7d63; margin-top: 2px; }
  .rc-row { display: flex; gap: 6px; }
  .rc-btn {
    flex: 1; cursor: pointer; appearance: none; padding: 6px 0; border-radius: 8px;
    background: rgba(6,24,15,0.6); border: 1px solid rgba(118,255,157,0.14);
    color: #9fcdb2; font-family: inherit; font-size: 12px; font-variant-numeric: tabular-nums;
    transition: border-color 140ms ease, background 140ms ease;
  }
  .rc-btn.is-on { border-color: #76ff9d; background: rgba(10,34,21,0.9); color: #d7ffe6; box-shadow: 0 0 16px -6px #76ff9d; }
  .rc-strats { display: grid; grid-template-columns: repeat(3, 1fr); gap: 6px; }
  .rc-strat {
    display: flex; flex-direction: column; gap: 3px; cursor: pointer; appearance: none; text-align: left;
    padding: 8px 9px; border-radius: 9px; background: rgba(6,24,15,0.6);
    border: 1px solid rgba(118,255,157,0.14); color: #9fcdb2; font-family: inherit;
    transition: border-color 140ms ease, background 140ms ease, transform 120ms ease;
  }
  .rc-strat:hover { transform: translateY(-1px); }
  .rc-strat.is-on { border-color: #76ff9d; background: rgba(10,34,21,0.92); box-shadow: 0 0 18px -6px #76ff9d; }
  .rc-strat-name { font-size: 12px; letter-spacing: 0.06em; color: #d7ffe6; }
  .rc-strat-blurb { font-size: 9.5px; line-height: 1.25; letter-spacing: 0.02em; color: #6f9d82; }
  .rc-hint { font-size: 10px; letter-spacing: 0.08em; color: #4d7d63; text-align: center; margin-top: 2px; }
`;
document.head.appendChild(style);

createRoot(document.getElementById('orb-root')!).render(<RadarCompare />);
