// rosterTree.ts — PURE grouping/nesting for the roster sidebar (no React, no
// Three, no colour-to-CSS). It turns the live forest / habit orbs into a list
// grouped by harness so a 20-25 agent fleet reads as a scannable tree:
//
//   • Radar  — group by harness; within a group, roots ordered working-first then
//     title, with each root's subagents nested beneath it (DFS, indented by depth).
//   • Habits — group the habit ORBS (read from the resolved layout nodes so a
//     row's id is exactly the node the forest rendered → a click selects that orb)
//     by harness, rows sorted by severity then frequency.
//
// Honest-viz: every agent/issue maps to a row, an orphaned subagent (parent absent)
// is still listed rather than dropped, and an empty harness yields no group. Harness
// identity (label/glyph/colour) comes from the single `harnessColors` source.

import type { RadarAgent, RadarStatus } from '@/viz/shared/types/radarTypes';
import { radarSubtitle } from '@/viz/shared/types/radarTypes';
import type { OrbLayout } from '@/viz/shared/types/orbTypes';
import { harnessColor } from '@/viz/shared/theme/harnessColors';

export type RosterRow = {
  id: string;
  title: string;
  subtitle: string | null;
  harness: string;
  /** Indent level: 0 = root agent / habit, 1 = subagent, … (radar only). */
  depth: number;
  /** Radar rows carry real liveness; the dot keys off it. */
  status?: RadarStatus;
  /** Habits rows carry the issue severity; the dot keys off it. */
  severity?: number;
};

export type HarnessGroup = {
  harness: string;
  label: string;
  glyph: string;
  color: string;
  rows: RosterRow[];
};

// Reading order: Claude, Codex, any other present harness (alpha), Unknown last.
function harnessRank(h: string): number {
  if (h === 'claude_code') return 0;
  if (h === 'codex') return 1;
  if (h === 'unknown') return 3;
  return 2;
}

function orderedHarnesses(present: Iterable<string>): string[] {
  return Array.from(new Set(present)).sort(
    (a, b) => harnessRank(a) - harnessRank(b) || a.localeCompare(b),
  );
}

function statusRank(s: RadarStatus): number {
  return s === 'working' ? 0 : s === 'idle' ? 1 : s === 'closed' ? 2 : 3;
}

function radarTitle(a: RadarAgent): string {
  const t = (a.nickname ?? a.label ?? '').trim();
  return t.length ? t : 'untitled agent';
}

function groupMeta(harness: string): Omit<HarnessGroup, 'rows'> {
  const c = harnessColor(harness);
  return { harness, label: c.label, glyph: c.glyph, color: c.hue };
}

export function buildRadarRoster(agents: RadarAgent[]): HarnessGroup[] {
  return orderedHarnesses(agents.map((a) => a.harness)).map((harness) => {
    const mine = agents.filter((a) => a.harness === harness);
    const inGroup = new Set(mine.map((a) => a.id));

    const childrenOf = new Map<string, RadarAgent[]>();
    for (const a of mine) {
      if (a.parentId && inGroup.has(a.parentId)) {
        const list = childrenOf.get(a.parentId) ?? [];
        list.push(a);
        childrenOf.set(a.parentId, list);
      }
    }

    const order = (xs: RadarAgent[]) =>
      [...xs].sort(
        (a, b) => statusRank(a.status) - statusRank(b.status) || radarTitle(a).localeCompare(radarTitle(b)),
      );

    const rows: RosterRow[] = [];
    const seen = new Set<string>();
    const visit = (a: RadarAgent) => {
      if (seen.has(a.id)) return;
      seen.add(a.id);
      rows.push({
        id: a.id,
        title: radarTitle(a),
        subtitle: radarSubtitle(a),
        harness,
        depth: Math.max(0, a.depth),
        status: a.status,
      });
      for (const kid of order(childrenOf.get(a.id) ?? [])) visit(kid);
    };

    // Roots (and orphans whose parent isn't in this group) first, DFS each; then a
    // final sweep guarantees no agent is ever dropped (e.g. a cyclic parent link).
    const roots = mine.filter((a) => !a.parentId || !inGroup.has(a.parentId));
    for (const r of order(roots)) visit(r);
    for (const a of order(mine)) visit(a);

    return { ...groupMeta(harness), rows };
  });
}

export function buildHabitsRoster(layout: OrbLayout): HarnessGroup[] {
  const issues = layout.nodes.filter((n) => n.kind === 'issue' && n.issue);
  return orderedHarnesses(issues.map((n) => n.harness)).map((harness) => {
    const rows: RosterRow[] = issues
      .filter((n) => n.harness === harness)
      .slice()
      .sort((a, b) => b.issue!.severity - a.issue!.severity || b.issue!.count - a.issue!.count)
      .map((n) => ({
        id: n.id,
        title: n.issue!.title,
        subtitle: `×${n.issue!.count} · sev ${n.issue!.severity}/5`,
        harness,
        depth: 0,
        severity: n.issue!.severity,
      }));
    return { ...groupMeta(harness), rows };
  });
}
