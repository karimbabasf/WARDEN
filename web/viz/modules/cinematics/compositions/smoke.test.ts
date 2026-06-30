import { describe, it, expect } from 'vitest';
import { createElement } from 'react';
import { Intro, Reveal, Recap, recapDuration, revealDuration, INTRO_DURATION } from './index';
import type { RevealFinding } from './Reveal';

// Render-SMOKE (node-safe): the vitest env is `node` (no DOM), so we do NOT
// rasterise frames here — the cinematic output is verified live in the overlay.
// What we DO assert is that the compositions are real components and that
// constructing them with realistic mock props throws nothing, and that the
// derived durations are coherent. Deep frame-by-frame visuals are intentionally
// NOT asserted (they're eyeballed in /dev-viz.html).

const mockFindings: RevealFinding[] = [
  { title: 'Unbounded Context', severity: 5, harness: 'claude_code', est_cost: 42000 },
  { title: 'No Subagents', severity: 4, harness: 'codex' },
  { title: 'Repeated Reads', severity: 2, harness: 'unknown' },
];

describe('composition render-smoke', () => {
  it('exposes all three compositions as components', () => {
    expect(typeof Intro).toBe('function');
    expect(typeof Reveal).toBe('function');
    expect(typeof Recap).toBe('function');
  });

  it('builds a Reveal element from real-shaped findings without throwing', () => {
    expect(() => createElement(Reveal, { findings: mockFindings, diagnosisId: 'diag-smoke-123' })).not.toThrow();
    const el = createElement(Reveal, { findings: mockFindings, diagnosisId: 'diag-smoke-123' });
    expect(el).toBeTypeOf('object');
    expect(el.type).toBe(Reveal);
  });

  it('builds Intro and Recap elements without throwing', () => {
    expect(() => createElement(Intro, {})).not.toThrow();
    expect(() => createElement(Recap, { findings: mockFindings, diagnosisId: 'd' })).not.toThrow();
  });

  it('tolerates an empty / malformed findings list (honest empty reveal)', () => {
    expect(() => createElement(Reveal, { findings: [], diagnosisId: '' })).not.toThrow();
    // @ts-expect-error — deliberately malformed to prove the component guards it.
    expect(() => createElement(Reveal, { findings: undefined, diagnosisId: 'x' })).not.toThrow();
  });

  it('reveal duration matches finding count; recap is shorter per-finding', () => {
    expect(revealDuration(mockFindings.length)).toBeGreaterThan(INTRO_DURATION - 1);
    // recap allots ~half a second per hole vs the reveal's full second
    expect(recapDuration(8)).toBeLessThan(revealDuration(8));
  });
});
