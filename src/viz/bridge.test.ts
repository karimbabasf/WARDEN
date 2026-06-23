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
});

describe('harnessTheme', () => {
  it('maps codex to violet', () => {
    expect(harnessTheme('codex').color).toBe('#b98cff');
    expect(harnessTheme('codex').glyph).toBe('▲');
  });

  it('maps claude_code to emerald', () => {
    expect(harnessTheme('claude_code').color).toBe('#3dffa0');
  });

  it('falls back to a neutral theme for unknown harnesses', () => {
    expect(harnessTheme('gemini').label).toBe('Unknown');
    expect(harnessTheme('gemini').color).toBe('#76ff9d');
  });
});
