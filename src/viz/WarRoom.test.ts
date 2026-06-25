// @vitest-environment jsdom

import { describe, expect, it } from 'vitest';
import { chromeModelForTab, isDiscoveryHomeDoubleClickAllowed, mergeArtifact } from './WarRoom';
import type { OrbSceneModel } from './orbTypes';
import type { RadarAgent, RadarSceneModel } from './radarTypes';
import type { Artifact } from './chrome';

function habitsModel(): OrbSceneModel {
  return {
    agents: [
      {
        id: 'habit-hub',
        harness: 'claude_code',
        label: 'Claude',
        glyph: '✶',
        color: '#ff8636',
        sessions: 3,
        eventCount: 12,
        totalLoad: 7,
      },
    ],
    issues: [
      {
        id: 'issue-1',
        agentId: 'habit-hub',
        harness: 'claude_code',
        patternId: 'no_delegation',
        title: 'No Delegation',
        count: 2,
        severity: 4,
        rationale: 'Search-heavy turns stayed in main context.',
        estCostTokens: 1200,
        estCostMinutes: 6,
        frequency: 0.2,
        confidence: 0.8,
        sessionIds: ['s1'],
        evidence: [],
      },
    ],
    links: [],
    guidance: { doItems: [], stopItems: [] },
  };
}

function radarAgent(partial: Partial<RadarAgent> & Pick<RadarAgent, 'id' | 'harness' | 'label'>): RadarAgent {
  return {
    origin: null,
    parentId: null,
    depth: 0,
    nickname: null,
    cwd: null,
    role: null,
    model: null,
    status: 'idle',
    fillPct: 0.2,
    contextTokens: 20000,
    maxTokens: 100000,
    composition: { exact: { cacheRead: 0, fresh: 0, output: 0 }, estimated: null },
    recentActivity: [],
    childCount: 0,
    startedAt: '',
    estCostUsd: null,
    ...partial,
  };
}

describe('isDiscoveryHomeDoubleClickAllowed', () => {
  it('allows double-click home while browsing with no selected globe or radar focus', () => {
    expect(
      isDiscoveryHomeDoubleClickAllowed({
        selectedId: null,
        focusDepth: 0,
        eventTarget: document.body,
      }),
    ).toBe(true);
  });

  it('blocks double-click home while a globe is selected', () => {
    expect(
      isDiscoveryHomeDoubleClickAllowed({
        selectedId: 'agent-1',
        focusDepth: 0,
        eventTarget: document.body,
      }),
    ).toBe(false);
  });

  it('blocks double-click home while radar focus is active', () => {
    expect(
      isDiscoveryHomeDoubleClickAllowed({
        selectedId: null,
        focusDepth: 1,
        eventTarget: document.body,
      }),
    ).toBe(false);
  });

  it('blocks double-click home from interactive controls', () => {
    const button = document.createElement('button');
    const input = document.createElement('input');

    expect(
      isDiscoveryHomeDoubleClickAllowed({
        selectedId: null,
        focusDepth: 0,
        eventTarget: button,
      }),
    ).toBe(false);
    expect(
      isDiscoveryHomeDoubleClickAllowed({
        selectedId: null,
        focusDepth: 0,
        eventTarget: input,
      }),
    ).toBe(false);
  });
});

describe('chromeModelForTab', () => {
  it('uses live radar agents for Radar chrome while preserving habit issues', () => {
    const habits = habitsModel();
    const radar: RadarSceneModel = {
      generatedAt: 'T',
      agents: [
        radarAgent({ id: 'live-agent-1', harness: 'codex', label: 'WARDEN' }),
        radarAgent({ id: 'live-agent-2', harness: 'claude_code', label: 'MOBIUS' }),
      ],
    };

    const model = chromeModelForTab('radar', habits, radar);

    expect(model.agents.map((a) => a.id)).toEqual(['live-agent-1', 'live-agent-2']);
    expect(model.agents.map((a) => a.label)).toEqual(['WARDEN', 'MOBIUS']);
    expect(model.issues).toBe(habits.issues);
  });

  it('leaves the habits model untouched for the Habits tab', () => {
    const habits = habitsModel();

    expect(chromeModelForTab('habits', habits, { generatedAt: 'T', agents: [] })).toBe(habits);
  });
});

describe('mergeArtifact', () => {
  const art = (over: Partial<Artifact> & Pick<Artifact, 'id' | 'status'>): Artifact => ({
    findingId: 'f1',
    kind: 'claude_md_guardrail',
    targetPath: '/tmp/CLAUDE.md',
    diff: '',
    block: '',
    appliedAt: null,
    backupPath: null,
    preImageSha256: null,
    postImageSha256: null,
    ...over,
  });

  it('prepends a new artifact newest-first', () => {
    const prev = [art({ id: 'a', status: 'applied' })];
    const next = mergeArtifact(prev, art({ id: 'b', status: 'applied' }));
    expect(next.map((a) => a.id)).toEqual(['b', 'a']);
  });

  it('replaces an existing row by id (no duplicate on re-apply/revert)', () => {
    const prev = [art({ id: 'a', status: 'applied' }), art({ id: 'b', status: 'applied' })];
    const next = mergeArtifact(prev, art({ id: 'a', status: 'reverted' }));
    expect(next.map((a) => a.id)).toEqual(['a', 'b']);
    expect(next.find((a) => a.id === 'a')?.status).toBe('reverted');
  });
});
