import { describe, it, expect } from 'vitest';
import { matchesFilter, targetDim, type EmphasisFilter } from './emphasis';

describe('matchesFilter', () => {
  it('matches any node when the filter is null', () => {
    expect(matchesFilter({ harness: 'claude_code' }, null)).toBe(true);
    expect(matchesFilter({ harness: null }, null)).toBe(true);
    expect(matchesFilter({}, null)).toBe(true);
  });

  it('matches only the selected harness', () => {
    const f: EmphasisFilter = { kind: 'harness', harness: 'claude_code' };
    expect(matchesFilter({ harness: 'claude_code' }, f)).toBe(true);
    expect(matchesFilter({ harness: 'codex' }, f)).toBe(false);
  });

  it('never matches a node with null or missing harness under a harness filter', () => {
    const f: EmphasisFilter = { kind: 'harness', harness: 'claude_code' };
    expect(matchesFilter({ harness: null }, f)).toBe(false);
    expect(matchesFilter({}, f)).toBe(false);
  });
});

describe('targetDim', () => {
  it('dims nothing when the filter is null', () => {
    expect(targetDim({ harness: 'claude_code' }, null)).toBe(0);
    expect(targetDim({}, null)).toBe(0);
  });

  it('keeps matches at 0 and dims non-matches to 1', () => {
    const f: EmphasisFilter = { kind: 'harness', harness: 'codex' };
    expect(targetDim({ harness: 'codex' }, f)).toBe(0);
    expect(targetDim({ harness: 'claude_code' }, f)).toBe(1);
    expect(targetDim({}, f)).toBe(1);
  });
});
