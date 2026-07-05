// @vitest-environment jsdom

import { describe, expect, it } from 'vitest';
import { radarChromeModel, humanizeBrainError, isDiscoveryHomeDoubleClickAllowed, mergeArtifact } from './WarRoom';
import type { OrbSceneModel } from './orbTypes';
import type { RadarAgent, RadarSceneModel } from './radarTypes';
import type { Artifact } from './chrome';

function radarAgent(partial: Partial<RadarAgent> & Pick<RadarAgent, 'id' | 'harness' | 'label'>): RadarAgent {
  return {
    origin: null,
    parentId: null,
    depth: 0,
    nickname: null,
    cwd: null,
    cwdPath: null,
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

describe('radarChromeModel', () => {
  it('derives one harness hub per real harness on screen (for the FilterBar chips)', () => {
    const radar: RadarSceneModel = {
      generatedAt: 'T',
      agents: [
        radarAgent({ id: 'live-agent-1', harness: 'codex', label: 'WARDEN', contextTokens: 20000 }),
        radarAgent({ id: 'live-agent-2', harness: 'claude_code', label: 'MOBIUS', contextTokens: 30000 }),
        radarAgent({ id: 'live-agent-3', harness: 'claude_code', label: 'child', contextTokens: 5000 }),
      ],
    };

    const model = radarChromeModel(radar);

    // one hub per distinct harness (deduped), never per-agent
    expect(model.agents.map((a: OrbSceneModel['agents'][number]) => a.harness).sort()).toEqual(['claude_code', 'codex']);
    // the claude hub aggregates both claude agents' load
    const claude = model.agents.find((a: OrbSceneModel['agents'][number]) => a.harness === 'claude_code');
    expect(claude?.sessions).toBe(2);
    expect(claude?.totalLoad).toBe(35000);
    // no issues are fabricated for the radar-derived chrome model
    expect(model.issues).toEqual([]);
  });

  it('yields no harness hubs when the fleet is empty', () => {
    expect(radarChromeModel({ generatedAt: 'T', agents: [] }).agents).toEqual([]);
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

// Raw Rust error strings (HTTP bodies, transport chains) must never hit the ask
// bar verbatim — the operator gets a plain sentence, with the raw tail kept short.
describe('humanizeBrainError', () => {
  it('maps auth failures to a key hint', () => {
    expect(humanizeBrainError('brain diagnostician HTTP 401 Unauthorized: {"error":"bad key"}')).toMatch(
      /api key/i,
    );
  });

  it('maps timeouts to a plain retry hint', () => {
    expect(humanizeBrainError('operation timed out after 75s')).toMatch(/took too long/i);
  });

  it('maps connection failures to a reachability hint', () => {
    expect(humanizeBrainError('error sending request: connection refused')).toMatch(/reach/i);
  });

  it('truncates unknown errors instead of dumping the full body', () => {
    const raw = 'X'.repeat(900);
    const out = humanizeBrainError(raw);
    expect(out.length).toBeLessThan(260);
  });
});
