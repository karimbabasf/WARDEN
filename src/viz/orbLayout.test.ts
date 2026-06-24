import { describe, expect, it } from 'vitest';
import { layoutOrbScene } from './orbLayout';
import type { OrbSceneModel } from './orbTypes';

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
        glyph: '▲',
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
});

