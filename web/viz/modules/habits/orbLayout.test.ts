import { describe, expect, it } from 'vitest';
import { layoutOrbScene } from './orbLayout';
import type { OrbSceneModel } from '@/viz/shared/types/orbTypes';

function scene(): OrbSceneModel {
  return {
    agents: [
      {
        id: 'claude_code',
        harness: 'claude_code',
        label: 'Claude',
        glyph: '◆',
        color: '#3dffa0',
        sessions: 3,
        eventCount: 30,
        totalLoad: 4,
      },
      {
        id: 'codex',
        harness: 'codex',
        label: 'Codex',
        glyph: '▣',
        color: '#b98cff',
        sessions: 2,
        eventCount: 12,
        totalLoad: 1,
      },
    ],
    issues: [
      {
        id: 'claude_code:CONTEXT_BLOAT',
        agentId: 'claude_code',
        harness: 'claude_code',
        patternId: 'CONTEXT_BLOAT',
        title: 'Context bloat',
        count: 3,
        severity: 4,
        rationale: 'r',
        estCostTokens: 10,
        estCostMinutes: 1,
        frequency: 0.6,
        confidence: 0.8,
        sessionIds: ['c1', 'c2', 'c3'],
        evidence: [],
      },
      {
        id: 'claude_code:NO_DELEGATION',
        agentId: 'claude_code',
        harness: 'claude_code',
        patternId: 'NO_DELEGATION',
        title: 'No delegation',
        count: 1,
        severity: 3,
        rationale: 'r',
        estCostTokens: 2,
        estCostMinutes: 1,
        frequency: 0.2,
        confidence: 0.7,
        sessionIds: ['c2'],
        evidence: [],
      },
      {
        id: 'codex:CONTEXT_BLOAT',
        agentId: 'codex',
        harness: 'codex',
        patternId: 'CONTEXT_BLOAT',
        title: 'Context bloat',
        count: 1,
        severity: 5,
        rationale: 'r',
        estCostTokens: 3,
        estCostMinutes: 1,
        frequency: 0.5,
        confidence: 0.9,
        sessionIds: ['x1'],
        evidence: [],
      },
    ],
    links: [
      { source: 'claude_code', target: 'claude_code:CONTEXT_BLOAT', kind: 'agent_issue' },
      { source: 'claude_code', target: 'claude_code:NO_DELEGATION', kind: 'agent_issue' },
      { source: 'codex', target: 'codex:CONTEXT_BLOAT', kind: 'agent_issue' },
    ],
    guidance: { doItems: [], stopItems: [] },
  };
}

// Many agents across both harnesses, each with a variable issue load — exercises
// harness zoning, collision-free hub packing and zone-bounds disjointness.
function multiHarnessScene(agentsPerHarness = 6, maxIssues = 8): OrbSceneModel {
  const harnesses = ['claude_code', 'codex'];
  const agents: OrbSceneModel['agents'] = [];
  const issues: OrbSceneModel['issues'] = [];
  const links: OrbSceneModel['links'] = [];
  for (const harness of harnesses) {
    for (let a = 0; a < agentsPerHarness; a++) {
      const id = `${harness}:agent${a}`;
      const load = (a % 5) + 1;
      agents.push({
        id,
        harness,
        label: id,
        glyph: harness === 'codex' ? '▣' : '◆',
        color: harness === 'codex' ? '#b98cff' : '#3dffa0',
        sessions: 1,
        eventCount: 10,
        totalLoad: load,
      });
      const n = (a % maxIssues) + 1; // 1..maxIssues issues, varies per agent
      for (let k = 0; k < n; k++) {
        const issueId = `${id}:P${k}`;
        issues.push({
          id: issueId,
          agentId: id,
          harness,
          patternId: `P${k}`,
          title: `Issue ${k}`,
          count: (k % 4) + 1,
          severity: (k % 5) + 1,
          rationale: 'r',
          estCostTokens: 1,
          estCostMinutes: 1,
          frequency: 0.1,
          confidence: 0.5,
          sessionIds: ['s'],
          evidence: [],
        });
        links.push({ source: id, target: issueId, kind: 'agent_issue' as const });
      }
    }
  }
  return { agents, issues, links, guidance: { doItems: [], stopItems: [] } };
}

// One busy agent with a spread of severities + counts — exercises the shell
// placement, severity→latitude ordering and non-intersection at real density.
function richScene(): OrbSceneModel {
  const sevs = [5, 5, 4, 3, 2, 1, 4, 2];
  const issues = sevs.map((severity, i) => ({
    id: `claude_code:P${i}`,
    agentId: 'claude_code',
    harness: 'claude_code',
    patternId: `P${i}`,
    title: `Issue ${i}`,
    count: (i % 4) + 1,
    severity,
    rationale: 'r',
    estCostTokens: 1,
    estCostMinutes: 1,
    frequency: 0.1,
    confidence: 0.5,
    sessionIds: ['s'],
    evidence: [],
  }));
  return {
    agents: [
      {
        id: 'claude_code',
        harness: 'claude_code',
        label: 'Claude',
        glyph: '◆',
        color: '#3dffa0',
        sessions: 1,
        eventCount: 10,
        totalLoad: issues.length,
      },
    ],
    issues,
    links: issues.map((i) => ({ source: 'claude_code', target: i.id, kind: 'agent_issue' as const })),
    guidance: { doItems: [], stopItems: [] },
  };
}

const dist = (a: { x: number; y: number; z: number }, b: { x: number; y: number; z: number }) =>
  Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z);

describe('layoutOrbScene', () => {
  it('is deterministic for the same scene model', () => {
    const a = layoutOrbScene(scene());
    const b = layoutOrbScene(scene());
    expect(a.nodes.map(n => [n.id, n.position.x, n.position.y, n.position.z])).toEqual(
      b.nodes.map(n => [n.id, n.position.x, n.position.y, n.position.z]),
    );
  });

  it('creates only issue-to-own-agent links', () => {
    const layout = layoutOrbScene(scene());
    expect(layout.links).toEqual([
      { source: 'claude_code', target: 'claude_code:CONTEXT_BLOAT', kind: 'agent_issue' },
      { source: 'claude_code', target: 'claude_code:NO_DELEGATION', kind: 'agent_issue' },
      { source: 'codex', target: 'codex:CONTEXT_BLOAT', kind: 'agent_issue' },
    ]);
    expect(layout.links.some(link => link.source === 'claude_code' && link.target === 'codex:CONTEXT_BLOAT')).toBe(false);
  });

  it('keeps larger persistent counts visually larger than smaller counts', () => {
    const layout = layoutOrbScene(scene());
    const large = layout.nodes.find(n => n.id === 'claude_code:CONTEXT_BLOAT')!;
    const small = layout.nodes.find(n => n.id === 'codex:CONTEXT_BLOAT')!;
    expect(large.radius).toBeGreaterThan(small.radius);
  });

  it("places an agent's issues on a spherical shell around its hub (true 3D, not one plane)", () => {
    const layout = layoutOrbScene(richScene());
    const hub = layout.nodes.find((n) => n.kind === 'hub' && n.agentId === 'claude_code')!;
    const issues = layout.nodes.filter((n) => n.kind === 'issue' && n.agentId === 'claude_code');
    const dists = issues.map((n) => dist(n.position, hub.position));
    const min = Math.min(...dists);
    const max = Math.max(...dists);
    expect(min).toBeGreaterThan(0.5); // off the hub
    expect(max - min).toBeLessThan(1e-6); // all on one shell radius
    // genuinely volumetric: spread across all three axes, not a flat ring
    const span = (sel: (p: { x: number; y: number; z: number }) => number) => {
      const vs = issues.map((n) => sel(n.position));
      return Math.max(...vs) - Math.min(...vs);
    };
    expect(span((p) => p.x)).toBeGreaterThan(0.5);
    expect(span((p) => p.y)).toBeGreaterThan(0.5);
    expect(span((p) => p.z)).toBeGreaterThan(0.5);
  });

  it('puts the worse of two issues higher (severity → latitude)', () => {
    const layout = layoutOrbScene(scene());
    const worse = layout.nodes.find((n) => n.id === 'claude_code:CONTEXT_BLOAT')!; // sev 4
    const milder = layout.nodes.find((n) => n.id === 'claude_code:NO_DELEGATION')!; // sev 3
    expect(worse.position.y).toBeGreaterThan(milder.position.y);
  });

  it('groups severity by height: mean latitude is non-increasing as severity drops', () => {
    const layout = layoutOrbScene(richScene());
    const issues = layout.nodes.filter((n) => n.kind === 'issue' && n.agentId === 'claude_code' && n.issue);
    const bySev = new Map<number, number[]>();
    for (const n of issues) {
      const s = n.issue!.severity;
      if (!bySev.has(s)) bySev.set(s, []);
      bySev.get(s)!.push(n.position.y);
    }
    const sevsDesc = [...bySev.keys()].sort((a, b) => b - a);
    const meanY = sevsDesc.map((s) => {
      const ys = bySev.get(s)!;
      return ys.reduce((a, b) => a + b, 0) / ys.length;
    });
    for (let i = 1; i < meanY.length; i++) {
      expect(meanY[i]).toBeLessThanOrEqual(meanY[i - 1] + 1e-9);
    }
  });

  it("never lets two of an agent's issues intersect", () => {
    const layout = layoutOrbScene(richScene());
    const issues = layout.nodes.filter((n) => n.kind === 'issue' && n.agentId === 'claude_code');
    for (let i = 0; i < issues.length; i++) {
      for (let j = i + 1; j < issues.length; j++) {
        expect(dist(issues[i].position, issues[j].position)).toBeGreaterThan(
          issues[i].radius + issues[j].radius,
        );
      }
    }
  });

  // ─── harness-zone + collision-free rewrite (task 4) ───

  it('no two nodes overlap across the whole scene (dist ≥ r1+r2)', () => {
    const layout = layoutOrbScene(multiHarnessScene());
    const ns = layout.nodes;
    const MARGIN = -1e-6; // allow exact tangency, reject real overlap
    for (let i = 0; i < ns.length; i++) {
      for (let j = i + 1; j < ns.length; j++) {
        const gap = dist(ns[i].position, ns[j].position) - (ns[i].radius + ns[j].radius);
        expect(gap).toBeGreaterThanOrEqual(MARGIN);
      }
    }
  });

  it('groups hubs into per-harness zones with disjoint X bounds', () => {
    const layout = layoutOrbScene(multiHarnessScene());
    const hubs = layout.nodes.filter((n) => n.kind === 'hub');
    // collect each harness zone's X extent from its hubs' footprints
    const byHarness = new Map<string, { min: number; max: number }>();
    for (const h of hubs) {
      const r = h.territoryRadius ?? h.radius;
      const lo = h.position.x - r;
      const hi = h.position.x + r;
      const cur = byHarness.get(h.harness);
      if (!cur) byHarness.set(h.harness, { min: lo, max: hi });
      else {
        cur.min = Math.min(cur.min, lo);
        cur.max = Math.max(cur.max, hi);
      }
    }
    expect(byHarness.size).toBeGreaterThanOrEqual(2);
    // every issue sits in the same harness X-band as its hub's zone
    const zoneOf = new Map(hubs.map((h) => [h.agentId, h.harness]));
    for (const n of layout.nodes.filter((n) => n.kind === 'issue')) {
      const harness = zoneOf.get(n.agentId)!;
      const band = byHarness.get(harness)!;
      expect(n.position.x).toBeGreaterThanOrEqual(band.min - 1e-9);
      expect(n.position.x).toBeLessThanOrEqual(band.max + 1e-9);
    }
    // zones' X intervals are pairwise disjoint
    const intervals = [...byHarness.values()].sort((a, b) => a.min - b.min);
    for (let i = 1; i < intervals.length; i++) {
      expect(intervals[i].min).toBeGreaterThan(intervals[i - 1].max);
    }
  });

  it('arranges zones on a shallow camera-facing arc (mild Z curvature, not one flat line)', () => {
    const layout = layoutOrbScene(multiHarnessScene());
    const hubs = layout.nodes.filter((n) => n.kind === 'hub');
    const cx = new Map<string, number[]>();
    const cz = new Map<string, number[]>();
    for (const h of hubs) {
      (cx.get(h.harness) ?? cx.set(h.harness, []).get(h.harness)!).push(h.position.x);
      (cz.get(h.harness) ?? cz.set(h.harness, []).get(h.harness)!).push(h.position.z);
    }
    const mean = (xs: number[]) => xs.reduce((a, b) => a + b, 0) / xs.length;
    const harnesses = [...cx.keys()];
    const zoneX = harnesses.map((h) => mean(cx.get(h)!));
    const zoneZ = harnesses.map((h) => mean(cz.get(h)!));
    const xSpread = Math.max(...zoneX) - Math.min(...zoneX);
    const zSpread = Math.max(...zoneZ) - Math.min(...zoneZ);
    // zones spread laterally in X (decodable side-by-side)
    expect(xSpread).toBeGreaterThan(2);
    // the arc gives zones a *real* depth difference — they are not all on one
    // flat Z line (the old layout pinned every zone to z≈0; this must change).
    expect(zSpread).toBeGreaterThan(0.6);
    // but the curvature is shallow: depth difference stays well under the
    // lateral spread, so it reads as an arc facing the camera, not a column.
    expect(zSpread).toBeLessThan(xSpread);
  });

  it('keeps the constellation compact, not a long receding line', () => {
    // 12 agents across 2 harnesses must pack into a bounded footprint. The old
    // single-axis packing blew X out to ~100 units; the zoned arc keeps it tight.
    const layout = layoutOrbScene(multiHarnessScene());
    const hubs = layout.nodes.filter((n) => n.kind === 'hub');
    const xs = hubs.map((h) => h.position.x);
    const xSpan = Math.max(...xs) - Math.min(...xs);
    expect(xSpan).toBeLessThan(60);
  });

  it('is deterministic at scale', () => {
    const a = layoutOrbScene(multiHarnessScene(30, 12));
    const b = layoutOrbScene(multiHarnessScene(30, 12));
    expect(a.nodes.map((n) => [n.id, n.position.x, n.position.y, n.position.z])).toEqual(
      b.nodes.map((n) => [n.id, n.position.x, n.position.y, n.position.z]),
    );
  });

  it('stays collision-free at scale (30 agents × up to 12 issues)', () => {
    const layout = layoutOrbScene(multiHarnessScene(30, 12));
    const ns = layout.nodes;
    expect(ns.length).toBeGreaterThan(100);
    for (let i = 0; i < ns.length; i++) {
      for (let j = i + 1; j < ns.length; j++) {
        const gap = dist(ns[i].position, ns[j].position) - (ns[i].radius + ns[j].radius);
        expect(gap).toBeGreaterThanOrEqual(-1e-6);
      }
    }
  });

  // ─── worst-case packing guard (T4 review I-1 / I-2) ───────────────────────
  // These scenarios push the separation pass hardest: all agents at maximum
  // footprint in a single zone, and an alternating big/tiny footprint mix.
  // Both must satisfy the scene-wide no-overlap invariant: ∀ pairs dist ≥ r1+r2.

  it('stays collision-free with 30 maximum-footprint agents in one zone', () => {
    // All agents same harness → single zone, maximum density packing stress-test.
    const agents: OrbSceneModel['agents'] = [];
    const issues: OrbSceneModel['issues'] = [];
    const links: OrbSceneModel['links'] = [];
    for (let a = 0; a < 30; a++) {
      const id = `claude_code:agent${a}`;
      agents.push({
        id,
        harness: 'claude_code',
        label: id,
        glyph: '◆',
        color: '#3dffa0',
        sessions: 1,
        eventCount: 10,
        totalLoad: 5,
      });
      // 12 issues each — maximum footprint for every agent
      for (let k = 0; k < 12; k++) {
        const issueId = `${id}:P${k}`;
        issues.push({
          id: issueId,
          agentId: id,
          harness: 'claude_code',
          patternId: `P${k}`,
          title: `Issue ${k}`,
          count: 10,
          severity: 5,
          rationale: 'r',
          estCostTokens: 1,
          estCostMinutes: 1,
          frequency: 0.1,
          confidence: 0.5,
          sessionIds: ['s'],
          evidence: [],
        });
        links.push({ source: id, target: issueId, kind: 'agent_issue' as const });
      }
    }
    const scene: OrbSceneModel = { agents, issues, links, guidance: { doItems: [], stopItems: [] } };
    const layout = layoutOrbScene(scene);
    const ns = layout.nodes;
    for (let i = 0; i < ns.length; i++) {
      for (let j = i + 1; j < ns.length; j++) {
        const gap = dist(ns[i].position, ns[j].position) - (ns[i].radius + ns[j].radius);
        expect(gap).toBeGreaterThanOrEqual(-1e-6);
      }
    }
  });

  it('stays collision-free with alternating max/min footprint agents (heterogeneous mix)', () => {
    // Alternating big (12 issues, high load) and tiny (1 issue, low load) agents
    // in a single zone — the hardest convergence case for a naive equal-push pass.
    const agents: OrbSceneModel['agents'] = [];
    const issues: OrbSceneModel['issues'] = [];
    const links: OrbSceneModel['links'] = [];
    for (let a = 0; a < 30; a++) {
      const id = `claude_code:agent${a}`;
      const isBig = a % 2 === 0;
      agents.push({
        id,
        harness: 'claude_code',
        label: id,
        glyph: '◆',
        color: '#3dffa0',
        sessions: 1,
        eventCount: 10,
        totalLoad: isBig ? 5 : 1,
      });
      const nIssues = isBig ? 12 : 1;
      for (let k = 0; k < nIssues; k++) {
        const issueId = `${id}:P${k}`;
        issues.push({
          id: issueId,
          agentId: id,
          harness: 'claude_code',
          patternId: `P${k}`,
          title: `Issue ${k}`,
          count: isBig ? 10 : 1,
          severity: isBig ? 5 : 1,
          rationale: 'r',
          estCostTokens: 1,
          estCostMinutes: 1,
          frequency: 0.1,
          confidence: 0.5,
          sessionIds: ['s'],
          evidence: [],
        });
        links.push({ source: id, target: issueId, kind: 'agent_issue' as const });
      }
    }
    const scene: OrbSceneModel = { agents, issues, links, guidance: { doItems: [], stopItems: [] } };
    const layout = layoutOrbScene(scene);
    const ns = layout.nodes;
    for (let i = 0; i < ns.length; i++) {
      for (let j = i + 1; j < ns.length; j++) {
        const gap = dist(ns[i].position, ns[j].position) - (ns[i].radius + ns[j].radius);
        expect(gap).toBeGreaterThanOrEqual(-1e-6);
      }
    }
  });
});

