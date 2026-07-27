import { describe, expect, it } from 'vitest';
import { humanizeSecs, isoRelative, isPast } from './observeFormat';

describe('humanizeSecs', () => {
  it('formats seconds, minutes, hours and days', () => {
    expect(humanizeSecs(0)).toBe('0s');
    expect(humanizeSecs(45)).toBe('45s');
    expect(humanizeSecs(90)).toBe('1m');
    expect(humanizeSecs(3700)).toBe('1h');
    expect(humanizeSecs(90000)).toBe('1d');
  });

  it('never goes negative or NaN', () => {
    expect(humanizeSecs(-5)).toBe('0s');
    expect(humanizeSecs(NaN)).toBe('0s');
  });
});

describe('isoRelative', () => {
  const now = Date.parse('2026-07-26T12:00:00Z');

  it('reads a past stamp as "X ago"', () => {
    expect(isoRelative('2026-07-26T11:55:00Z', now)).toBe('5m ago');
  });

  it('reads a future stamp as "in X"', () => {
    expect(isoRelative('2026-07-26T12:05:00Z', now)).toBe('in 5m');
  });

  it('collapses a near-now future stamp to "now"', () => {
    expect(isoRelative('2026-07-26T12:00:02Z', now)).toBe('now');
  });

  it('returns empty string for an unparseable stamp', () => {
    expect(isoRelative('not a date', now)).toBe('');
    expect(isoRelative('', now)).toBe('');
  });
});

describe('isPast', () => {
  const now = Date.parse('2026-07-26T12:00:00Z');

  it('treats an earlier stamp as past', () => {
    expect(isPast('2026-07-26T11:00:00Z', now)).toBe(true);
  });

  it('treats a later stamp as not past', () => {
    expect(isPast('2026-07-26T13:00:00Z', now)).toBe(false);
  });

  it('treats an unparseable stamp as past (the safe reading for a TTL check)', () => {
    expect(isPast('garbage', now)).toBe(true);
  });
});
