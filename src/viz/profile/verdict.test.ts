// @vitest-environment node
//
// verdict.test.ts — the pure verdict-line generator that captions THE SCORE hero.
// It reads the efficiency rubric families and names the operator's single
// strongest and single weakest family in plain language. TDD: this file is the
// contract; `verdict.ts` implements it.

import { describe, expect, it } from 'vitest';
import { verdictLine, familyLabel, strongestWeakest } from './verdict';
import type { EfficiencyFamily, EfficiencyScore } from './types';

function fam(key: string, sub_score: number, weight = 1): EfficiencyFamily {
  return { key, sub_score, weight };
}

function score(families: EfficiencyFamily[]): EfficiencyScore {
  return { headline: 0.5, rubric_version: 'v1', families, session_count: 10 };
}

describe('familyLabel', () => {
  it('humanizes a known snake_case rubric key', () => {
    expect(familyLabel('context_hygiene')).toBe('context hygiene');
    expect(familyLabel('verification_discipline')).toBe('verification discipline');
  });

  it('passes an already-human label through untouched', () => {
    expect(familyLabel('Delegation')).toBe('Delegation');
  });

  it('is safe on empty / whitespace', () => {
    expect(familyLabel('')).toBe('');
    expect(familyLabel('   ')).toBe('');
  });
});

describe('strongestWeakest', () => {
  it('names the max-score and min-score families', () => {
    const fams = [
      fam('delegation', 0.9),
      fam('context_hygiene', 0.3),
      fam('verification', 0.6),
    ];
    const r = strongestWeakest(fams);
    expect(r?.strongest.key).toBe('delegation');
    expect(r?.weakest.key).toBe('context_hygiene');
  });

  it('returns null for an empty family list', () => {
    expect(strongestWeakest([])).toBeNull();
  });

  it('ignores non-finite sub_scores rather than picking them', () => {
    const fams = [
      fam('a', Number.NaN),
      fam('b', 0.5),
      fam('c', 0.2),
    ];
    const r = strongestWeakest(fams);
    expect(r?.strongest.key).toBe('b');
    expect(r?.weakest.key).toBe('c');
  });

  it('when only one family exists, strongest and weakest are the same', () => {
    const r = strongestWeakest([fam('solo', 0.7)]);
    expect(r?.strongest.key).toBe('solo');
    expect(r?.weakest.key).toBe('solo');
  });
});

describe('verdictLine', () => {
  it('names strongest and weakest in a readable sentence', () => {
    const s = score([
      fam('delegation', 0.92),
      fam('context_hygiene', 0.28),
      fam('verification', 0.55),
    ]);
    const line = verdictLine(s);
    expect(line).toContain('delegation'); // strongest
    expect(line).toContain('context hygiene'); // weakest, humanized
    expect(line.toLowerCase()).toContain('strongest');
    expect(line.toLowerCase()).toContain('lose most');
  });

  it('falls back gracefully with no families', () => {
    const line = verdictLine(score([]));
    expect(typeof line).toBe('string');
    expect(line.length).toBeGreaterThan(0);
    expect(line).not.toContain('undefined');
    expect(line).not.toContain('NaN');
  });

  it('falls back gracefully with a null score', () => {
    const line = verdictLine(null);
    expect(typeof line).toBe('string');
    expect(line.length).toBeGreaterThan(0);
  });

  it('collapses to a single-strength phrasing when strongest === weakest', () => {
    const line = verdictLine(score([fam('solo_family', 0.6)]));
    expect(line).toContain('solo family');
    expect(line).not.toContain('undefined');
  });
});
