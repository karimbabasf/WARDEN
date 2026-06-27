// Sidebar.tsx — the toggle roster dock (left). It lists every globe as a scannable
// list so a 20-25 agent fleet is navigable without hunting in 3D: Radar shows live
// agents grouped by harness with subagents nested under their root; Habits shows the
// habit orbs grouped by harness. Closed by default; the ≡ button (in WarRoom) and the
// header ✕ both call `onToggle`. A row click calls `onPick(id)`, which WarRoom turns
// into the existing select → camera-dive → detail-dock flow (no new selection state).
//
// Pure presentation: the grouping/nesting is `rosterTree`; the dot keys off the row's
// real liveness (radar) or severity (habits) — colour is paired with the status word
// in the aria-label so it is never the only signal.

import { type CSSProperties } from 'react';
import type { ConstellationTab } from './NavBar';
import type { HarnessGroup, RosterRow } from '@/viz/modules/radar/rosterTree';
import { severityColor } from '@/viz/shared/theme/harnessTheme';

function Row({
  row,
  color,
  selected,
  onPick,
}: {
  row: RosterRow;
  color: string;
  selected: boolean;
  onPick: (id: string) => void;
}) {
  const dotClass = row.status ? `wd-roster-dot is-${row.status}` : 'wd-roster-dot';
  const dotStyle =
    row.severity !== undefined
      ? ({ background: severityColor(row.severity), color: severityColor(row.severity) } as CSSProperties)
      : undefined;
  const stateWord = row.status ?? (row.severity !== undefined ? `severity ${row.severity}` : '');
  return (
    <button
      type="button"
      className={`wd-roster-row${selected ? ' is-selected' : ''}`}
      data-roster-id={row.id}
      data-depth={row.depth}
      aria-current={selected ? 'true' : undefined}
      aria-label={`${row.title}${stateWord ? ` — ${stateWord}` : ''}`}
      title={row.subtitle ? `${row.title} · ${row.subtitle}` : row.title}
      style={{ '--harness': color, paddingLeft: `${10 + row.depth * 14}px` } as CSSProperties}
      onClick={() => onPick(row.id)}
    >
      <span className={dotClass} style={dotStyle} aria-hidden="true" />
      <span className="wd-roster-text">
        <span className="wd-roster-title">{row.title}</span>
        {row.subtitle && <span className="wd-roster-sub">{row.subtitle}</span>}
      </span>
    </button>
  );
}

export function Sidebar({
  open,
  displayTab,
  groups,
  headerCount,
  selectedId,
  onPick,
  onToggle,
}: {
  open: boolean;
  displayTab: ConstellationTab;
  groups: HarnessGroup[];
  headerCount: string;
  selectedId: string | null;
  onPick: (id: string) => void;
  onToggle: () => void;
}) {
  const title = displayTab === 'radar' ? 'Roster' : 'Habits';
  return (
    <aside id="wd-roster" className="wd-sidebar" data-open={open} aria-hidden={!open} aria-label={`${title} list`}>
      <div className="wd-sidebar-head">
        <div className="wd-sidebar-titles">
          <span className="wd-sidebar-title">{title}</span>
          <span className="wd-sidebar-count">{headerCount}</span>
        </div>
        <button
          type="button"
          className="wd-sidebar-close"
          onClick={onToggle}
          aria-label="Collapse roster"
          title="Collapse"
        >
          ✕
        </button>
      </div>
      <div className="wd-sidebar-body">
        {groups.length === 0 ? (
          <div className="wd-sidebar-empty">{displayTab === 'radar' ? 'No live agents' : 'No habits mapped'}</div>
        ) : (
          groups.map((g) => (
            <section className="wd-roster-group" data-group={g.harness} key={g.harness} role="group" aria-label={g.label}>
              <div className="wd-roster-grouphead" style={{ '--harness': g.color } as CSSProperties}>
                <span className="wd-roster-groupglyph" aria-hidden="true">{g.glyph}</span>
                <span className="wd-roster-grouplabel">{g.label}</span>
                <span className="wd-roster-groupcount">{g.rows.length}</span>
              </div>
              {g.rows.map((r) => (
                <Row key={r.id} row={r} color={g.color} selected={selectedId === r.id} onPick={onPick} />
              ))}
            </section>
          ))
        )}
      </div>
    </aside>
  );
}

export default Sidebar;
