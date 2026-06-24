// radarReal.tsx — THROWAWAY diagnostic harness (safe to delete).
//
// Identical to radarLab.tsx but fed the REAL forest dumped by the radar_probe
// example (`realRadar.json` = the exact bytes `get_radar_state` returns over IPC),
// run through the same `normalizeRadarState` seam the live app uses. Proves the live
// radar computation renders as globes in the real components, with zero backend.

import { useCallback, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { RadarConstellation } from '../RadarConstellation';
import { RadarDetailPanel } from '../RadarDetailPanel';
import { normalizeRadarState } from '../radarTypes';
import { isFlatAgent } from '../radarLayout';
import { radarHarness } from '../radarTheme';
import type { LayoutNode } from '../orbTypes';
import type { RadarAgent } from '../radarTypes';
import RAW from './realRadar.json';
import '../../style.css';

const FOREST = normalizeRadarState(RAW as unknown);
const DEFAULT_SELECTED = FOREST.agents[0]?.id ?? null;

function RadarReal() {
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

      <div className={`wd-inspector wd-radar-dock ${selectedAgent ? 'is-open' : ''}`}>
        {selectedAgent ? (
          <RadarDetailPanel agent={selectedAgent} children={children} onJumpTo={onJumpTo} onClose={onClear} />
        ) : null}
      </div>

      <div className="radar-lab-picker">
        <div className="radar-lab-mark">
          <span className="sig">WARDEN</span>
          <span className="ver">radar · LIVE data ({FOREST.agents.length})</span>
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

createRoot(document.getElementById('orb-root')!).render(<RadarReal />);
