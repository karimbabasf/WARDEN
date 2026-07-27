import { describe, expect, it } from 'vitest';
import {
  grantStateMeta,
  normalizeGrantRow,
  normalizeNewGrant,
  normalizePeerRow,
  normalizeSharingStatus,
  readableError,
  toGrantState,
} from './observeTypes';

describe('toGrantState', () => {
  it('accepts the four known states', () => {
    expect(toGrantState('pending')).toBe('pending');
    expect(toGrantState('redeemed')).toBe('redeemed');
    expect(toGrantState('revoked')).toBe('revoked');
    expect(toGrantState('expired')).toBe('expired');
  });

  it('falls back to the most restrictive reading for anything unrecognised', () => {
    expect(toGrantState('made-up')).toBe('expired');
    expect(toGrantState(undefined)).toBe('expired');
    expect(toGrantState(42)).toBe('expired');
  });
});

it('grantStateMeta pairs every state with a glyph and a label', () => {
  for (const state of ['pending', 'redeemed', 'revoked', 'expired'] as const) {
    const meta = grantStateMeta(state);
    expect(meta.glyph.length).toBeGreaterThan(0);
    expect(meta.label.length).toBeGreaterThan(0);
  }
});

describe('normalizeSharingStatus', () => {
  it('normalizes a live status', () => {
    const s = normalizeSharingStatus({ sharing: true, endpointId: 'ep1', fingerprint: 'AB12:CD34', liveObservers: 2 });
    expect(s).toEqual({ sharing: true, endpointId: 'ep1', fingerprint: 'AB12:CD34', liveObservers: 2 });
  });

  it('defaults a malformed payload to "not sharing"', () => {
    const s = normalizeSharingStatus({});
    expect(s.sharing).toBe(false);
    expect(s.endpointId).toBeNull();
    expect(s.liveObservers).toBe(0);
  });
});

it('normalizeNewGrant reads the once-shown token straight through', () => {
  const g = normalizeNewGrant({ grantId: 'g1', token: 'secret-token', expiresAt: '2026-07-26T12:00:00Z' });
  expect(g).toEqual({ grantId: 'g1', token: 'secret-token', expiresAt: '2026-07-26T12:00:00Z' });
});

it('normalizeGrantRow coerces an unknown state to expired and defaults a missing label', () => {
  const row = normalizeGrantRow({ grantId: 'g1', state: 'not-a-real-state' });
  expect(row.state).toBe('expired');
  expect(row.label).toBe('untitled');
  expect(row.connected).toBe(false);
});

it('normalizePeerRow defaults a missing host label rather than leaving it blank', () => {
  const row = normalizePeerRow({ peerId: 'p1' });
  expect(row.hostLabel).toBe('unnamed host');
  expect(row.connected).toBe(false);
  expect(row.error).toBeNull();
});

describe('readableError', () => {
  it('extracts the message from an Error', () => {
    expect(readableError(new Error('boom'))).toBe('boom');
  });

  it('passes a plain string through (the common Tauri command-failure shape)', () => {
    expect(readableError('token expired')).toBe('token expired');
  });

  it('falls back to readable text for anything else, never leaving a caller with undefined', () => {
    expect(readableError(undefined)).toContain('backend');
    expect(readableError({ weird: true })).toContain('backend');
  });
});
