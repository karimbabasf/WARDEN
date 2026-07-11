// @vitest-environment jsdom

import { describe, expect, it } from 'vitest';
import { activeFor, isDiscoveryHomeDoubleClickAllowed, RADAR_VISIBLE_PULL_MS } from './WarRoom';

describe('activeFor', () => {
  it('pauses only on minimize', () => {
    expect(activeFor(true, false, true)).toBe(false);
    expect(activeFor(false, false, true)).toBe(false);
  });

  it('stays active while summoned even if the page reports hidden', () => {
    expect(activeFor(true, true, false)).toBe(true);
  });

  it('keys off page visibility when not summoned (dev/browser)', () => {
    expect(activeFor(false, false, false)).toBe(true);
    expect(activeFor(false, true, false)).toBe(false);
  });
});

describe('isDiscoveryHomeDoubleClickAllowed', () => {
  it('is blocked while an agent is selected or the camera is focused in', () => {
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: 'a1', focusDepth: 0, eventTarget: null })).toBe(false);
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 2, eventTarget: null })).toBe(false);
  });

  it('is allowed on the empty void', () => {
    const div = document.createElement('div');
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 0, eventTarget: div })).toBe(true);
  });

  it('is blocked when the double-click lands on a control', () => {
    const btn = document.createElement('button');
    expect(isDiscoveryHomeDoubleClickAllowed({ selectedId: null, focusDepth: 0, eventTarget: btn })).toBe(false);
  });
});

describe('RADAR_VISIBLE_PULL_MS', () => {
  it('is a light polling cadence, not a busy loop', () => {
    expect(RADAR_VISIBLE_PULL_MS).toBeGreaterThan(0);
    expect(RADAR_VISIBLE_PULL_MS).toBeLessThan(5000);
  });
});
