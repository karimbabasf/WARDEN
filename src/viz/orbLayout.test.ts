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
});

