import { describe, it, expect } from 'vitest';
import { createBridge, type SceneState } from './bridge';
import { harnessTheme } from './harnessTheme';

// `createBridge` takes the Tauri `listen` so the bridge can self-wire in the
// app, but every test drives it synchronously through `ingest(name, payload)`
// — no Tauri runtime required. We pass a no-op listen stub.
const noopListen = (async () => () => {}) as unknown as Parameters<typeof createBridge>[0];

function snapshot(bridge: ReturnType<typeof createBridge>): SceneState {
  let latest!: SceneState;
  const unsub = bridge.subscribe(s => {
    latest = s;
  });
  unsub();
  return latest;
}

function candidate(patternId: string, harness = 'claude_code', severityHint = 2) {
  return { pattern_id: patternId, session_id: `sess-${patternId}`, harness, severity_hint: severityHint };
}

describe('bridge reducer', () => {
  it('stores the persistent orb scene without disturbing live run signals', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('candidates_nominated', { candidates: [candidate('p1')] });
    bridge.ingest('orb_scene_ready', {
      agents: [
        {
          id: 'claude_code',
          harness: 'claude_code',
          label: 'Claude',
          glyph: '◆',
          color: '#3dffa0',
          sessions: 2,
          event_count: 10,
          total_load: 3,
        },
      ],
      issues: [
        {
          id: 'claude_code:CONTEXT_BLOAT',
          agent_id: 'claude_code',
          harness: 'claude_code',
          pattern_id: 'CONTEXT_BLOAT',
          title: 'Context bloat',
          count: 3,
          severity: 4,
          rationale: 'Main-context discovery is recurring.',
          est_cost_tokens: 12000,
          est_cost_minutes: 8,
          frequency: 0.75,
          confidence: 0.82,
          session_ids: ['c1', 'c2', 'c3'],
          evidence: [],
          finding_id: 'f-context',
          verifier_verdict: 'confirmed',
          status: 'confirmed',
        },
      ],
      links: [{ source: 'claude_code', target: 'claude_code:CONTEXT_BLOAT', kind: 'agent_issue' }],
      guidance: { do_items: ['Delegate broad search.'], stop_items: ['Stop flooding main context.'] },
    });
    const s = snapshot(bridge);
    expect(s.phase).toBe('war');
    expect(s.candidates).toHaveLength(1);
    expect(s.orbScene?.agents[0].totalLoad).toBe(3);
    expect(s.orbScene?.issues[0]).toMatchObject({
      id: 'claude_code:CONTEXT_BLOAT',
      agentId: 'claude_code',
      patternId: 'CONTEXT_BLOAT',
      count: 3,
      severity: 4,
    });
    expect(s.orbScene?.guidance.doItems).toEqual(['Delegate broad search.']);
  });

  it('flows a radar_state payload into SceneState.radarScene (normalized, camelCase)', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('radar_scene_ready', {
      generatedAt: '2026-06-23T22:50:17Z',
      agents: [
        {
          id: 'root-1',
          harness: 'claude_code',
          parentId: null,
          depth: 0,
          label: 'warden',
          status: 'working',
          contextTokens: 120000,
          maxTokens: 200000,
          fillPct: 0.6,
          composition: { exact: { cacheRead: 90000, fresh: 12000, output: 2620 }, estimated: null },
          recentActivity: [],
          childCount: 1,
          startedAt: '2026-06-23T22:00:00Z',
          estCostUsd: 0.42,
        },
        {
          id: 'sub-1',
          harness: 'claude_code',
          parentId: 'root-1',
          depth: 1,
          label: 'Explore · map frontend',
          status: 'idle',
          contextTokens: 8000,
          maxTokens: 200000,
          fillPct: 0.04,
          composition: { exact: { cacheRead: 0, fresh: 0, output: 0 }, estimated: null },
          recentActivity: [],
          childCount: 0,
          startedAt: '2026-06-23T22:40:00Z',
          estCostUsd: null,
        },
      ],
    });
    const s = snapshot(bridge);
    expect(s.radarScene?.generatedAt).toBe('2026-06-23T22:50:17Z');
    expect(s.radarScene?.agents).toHaveLength(2);
    expect(s.radarScene?.agents[0]).toMatchObject({ id: 'root-1', depth: 0, parentId: null, contextTokens: 120000 });
    expect(s.radarScene?.agents[0].fillPct).toBeCloseTo(0.6);
    expect(s.radarScene?.agents[1]).toMatchObject({ id: 'sub-1', depth: 1, parentId: 'root-1' });
  });

  it('reset preserves the persistent radar scene (live forest, not a run signal)', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('radar_scene_ready', {
      agents: [{ id: 'r', harness: 'codex', status: 'working', contextTokens: 1000 }],
    });
    bridge.ingest('candidates_nominated', { candidates: [candidate('p1')] });
    bridge.reset();
    const s = snapshot(bridge);
    expect(s.candidates).toHaveLength(0);
    expect(s.radarScene?.agents[0].id).toBe('r');
  });

  it('spawns one node per nominated candidate', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('candidates_nominated', {
      candidates: [candidate('p1'), candidate('p2', 'codex'), candidate('p3')],
    });
    const s = snapshot(bridge);
    expect(s.candidates).toHaveLength(3);
    expect(s.phase).toBe('war');
    expect(s.candidates.map(c => c.harness)).toEqual(['claude_code', 'codex', 'claude_code']);
  });

  it('records a confirmed verdict keyed by finding id', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('candidates_nominated', { candidates: [candidate('p1')] });
    bridge.ingest('finding_verdict', {
      finding_id: 'f1',
      pattern_id: 'p1',
      harness: 'claude_code',
      verdict: 'confirmed',
      severity: 4,
    });
    const s = snapshot(bridge);
    expect(s.verdicts['f1'].verdict).toBe('confirmed');
    expect(s.verdicts['f1'].severity).toBe(4);
  });

  it('flags a refuted verdict and moves to reveal on diagnosis_ready', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('candidates_nominated', { candidates: [candidate('p1'), candidate('p2')] });
    bridge.ingest('finding_verdict', {
      finding_id: 'f1',
      pattern_id: 'p1',
      harness: 'claude_code',
      verdict: 'refuted',
      severity: 1,
    });
    expect(snapshot(bridge).phase).toBe('war');
    bridge.ingest('diagnosis_ready', { id: 'd1', finding_count: 1 });
    const s = snapshot(bridge);
    expect(s.phase).toBe('reveal');
    expect(s.verdicts['f1'].verdict).toBe('refuted');
    // a refuted finding is recorded as refuted, never silently dropped
    expect(Object.values(s.verdicts).some(v => v.verdict === 'refuted')).toBe(true);
  });

  it('records fugu_delta pulses and fugu_usage per stage (honest viz signals)', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('fugu_delta', { stage: 'Diagnostician', delta: 'looking…' });
    bridge.ingest('fugu_usage', {
      stage: 'Diagnostician',
      input_tokens: 1200,
      output_tokens: 300,
      orchestration_input_tokens: 80,
      orchestration_output_tokens: 40,
    });
    const s = snapshot(bridge);
    expect(s.pulses.length).toBeGreaterThan(0);
    expect(s.usage['Diagnostician']).toEqual({ in: 1200, out: 300, orchIn: 80, orchOut: 40 });
  });

  it('does not fabricate a usage pulse for all-zero Chat Completions usage', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('fugu_usage', {
      stage: 'Verifier',
      input_tokens: 0,
      output_tokens: 0,
      orchestration_input_tokens: 0,
      orchestration_output_tokens: 0,
    });

    const s = snapshot(bridge);
    expect(s.usage['Verifier']).toEqual({ in: 0, out: 0, orchIn: 0, orchOut: 0 });
    expect(s.pulses).toHaveLength(0);
  });

  it('uses plain token weight when Chat Completions has no orchestration tokens', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('fugu_usage', {
      stage: 'Coach',
      input_tokens: 800,
      output_tokens: 200,
      orchestration_input_tokens: 0,
      orchestration_output_tokens: 0,
    });

    const s = snapshot(bridge);
    expect(s.usage['Coach']).toEqual({ in: 800, out: 200, orchIn: 0, orchOut: 0 });
    expect(s.pulses).toHaveLength(1);
    expect(s.pulses[0].intensity).toBeGreaterThan(0);
  });

  it('clamps nodes at 24 and counts the clustered overflow', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('candidates_nominated', {
      candidates: Array.from({ length: 30 }, (_, i) => candidate(`p${i}`)),
    });
    const s = snapshot(bridge);
    expect(s.candidates).toHaveLength(24);
    expect(s.clustered).toBe(6);
  });

  it('reset returns to idle with no candidates', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('candidates_nominated', { candidates: [candidate('p1')] });
    bridge.reset();
    const s = snapshot(bridge);
    expect(s.phase).toBe('idle');
    expect(s.candidates).toHaveLength(0);
    expect(Object.keys(s.verdicts)).toHaveLength(0);
  });

  it('reset preserves persistent orb scene and summon state while clearing live signals', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('orb_scene_ready', {
      agents: [{ id: 'codex', harness: 'codex', label: 'Codex', glyph: '▣', color: '#b98cff' }],
      issues: [],
      links: [],
      guidance: {},
    });
    bridge.ingest('warden_hotkey', {});
    bridge.ingest('candidates_nominated', { candidates: [candidate('p1')] });
    bridge.ingest('fugu_delta', { stage: 'Diagnostician', delta: 'x' });

    bridge.reset();

    const s = snapshot(bridge);
    expect(s.phase).toBe('idle');
    expect(s.candidates).toHaveLength(0);
    expect(s.pulses).toHaveLength(0);
    expect(s.orbScene?.agents[0].id).toBe('codex');
    expect(s.summoned).toBe(true);
  });

  it('wakes on warden_hotkey and pauses on warden_dismiss (overlay summon signal)', () => {
    const bridge = createBridge(noopListen);
    expect(snapshot(bridge).summoned).toBeFalsy();
    bridge.ingest('warden_hotkey', {});
    expect(snapshot(bridge).summoned).toBe(true);
    bridge.ingest('warden_dismiss', {});
    expect(snapshot(bridge).summoned).toBe(false);
  });

  it('clears the minimized pause when the overlay is summoned again', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('warden_hotkey', {});
    bridge.ingest('warden_minimized', {});

    bridge.ingest('warden_hotkey', {});

    const s = snapshot(bridge);
    expect(s.summoned).toBe(true);
    expect(s.minimized).toBe(false);
  });
});

describe('harnessTheme', () => {
  it('maps codex to its cyan-ice mark', () => {
    expect(harnessTheme('codex').color).toBe('#4fc9ff');
    expect(harnessTheme('codex').glyph).toBe('▣');
  });

  it('maps claude_code to its tangy-tangerine brand colour', () => {
    expect(harnessTheme('claude_code').color).toBe('#ff8636');
  });

  it('falls back to a neutral theme for unknown harnesses', () => {
    expect(harnessTheme('gemini').label).toBe('Unknown');
    expect(harnessTheme('gemini').color).toBe('#8fa0b8');
  });
});
