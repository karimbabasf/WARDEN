// @vitest-environment jsdom
//
// The rack's half of the third state. The globe carries the alarm; the strip carries the
// ANSWER to "which one, and what does it want?", and that half has to survive a fold
// (the census line is the whole board when the rack is a tab).
//
// Rendered under jsdom with react-dom/client + act, the house no-deps style.

import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { FleetRail, fleetSummary } from './FleetRail';
import { normalizeRadarState } from '@/viz/shared/types/radarTypes';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';

function agents(raw: Array<Record<string, unknown>>): RadarAgent[] {
  return normalizeRadarState({ agents: raw }).agents;
}

let host: HTMLDivElement | null = null;
let root: Root | null = null;

function render(list: RadarAgent[]): HTMLDivElement {
  host = document.createElement('div');
  document.body.appendChild(host);
  root = createRoot(host);
  act(() => {
    root!.render(
      <FleetRail
        agents={list}
        selectedId={null}
        onSelect={() => {}}
        collapsed={false}
        onToggleCollapsed={() => {}}
        hidden={[]}
        onHide={() => {}}
        onRestore={() => {}}
        onRestoreAll={() => {}}
      />,
    );
  });
  return host;
}

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  host = null;
  root = null;
});

describe('fleetSummary', () => {
  it('counts waiting sessions separately from working ones', () => {
    expect(fleetSummary(5, 2, 2)).toBe('5 sessions · 2 working · 2 waiting');
  });

  it('omits a zero count rather than printing "0 waiting"', () => {
    expect(fleetSummary(3, 1, 0)).toBe('3 sessions · 1 working');
    expect(fleetSummary(3, 0, 1)).toBe('3 sessions · 1 waiting');
    expect(fleetSummary(1, 0, 0)).toBe('1 session');
  });

  it('keeps working before waiting, so the line ends on the actionable count', () => {
    const s = fleetSummary(4, 1, 1);
    expect(s.indexOf('working')).toBeLessThan(s.indexOf('waiting'));
  });
});

describe('an awaiting strip', () => {
  it('says Waiting and flags itself for the alert styling', () => {
    const el = render(
      agents([
        { id: 'w', harness: 'codex', status: 'working', label: 'busy-one' },
        { id: 'a', harness: 'claude_code', status: 'awaiting', awaitingReason: 'approval', label: 'blocked-one' },
      ]),
    );
    const strip = el.querySelector<HTMLElement>('[data-strip="a"]');
    expect(strip?.dataset.status).toBe('awaiting');
    expect(strip?.querySelector('.wd-strip-status')?.textContent).toBe('Waiting');
    expect(strip?.querySelector('.wd-strip-status')?.className).toContain('is-awaiting');
  });

  it('replaces "No action in flight" with what it is waiting for', () => {
    const el = render(
      agents([
        { id: 'a', harness: 'codex', status: 'awaiting', awaitingReason: 'question', label: 'asked' },
      ]),
    );
    const line = el.querySelector('[data-strip="a"] .wd-strip-action-label')?.textContent;
    // "No action in flight" is true here and useless: it is the sentence for a session
    // you can ignore, and this is the one session you cannot.
    expect(line).toBe('Asked you a question');
    expect(el.textContent).not.toContain('No action in flight');
  });

  it('quotes the open prompt when the harness gave it one', () => {
    const el = render(
      agents([
        {
          id: 'a',
          harness: 'claude_code',
          status: 'awaiting',
          awaitingReason: 'question',
          label: 'asked',
          currentAction: {
            kind: 'ask',
            tool: 'AskUserQuestion',
            label: 'Cut over now, or stage it behind a flag?',
            target: null,
            startedAt: new Date().toISOString(),
            elapsedMs: 74_000,
          },
        },
      ]),
    );
    expect(el.querySelector('[data-strip="a"] .wd-strip-action-label')?.textContent).toBe(
      'Cut over now, or stage it behind a flag?',
    );
  });

  it('puts the waiting count in the census, including when the rack is folded', () => {
    const list = agents([
      { id: 'w', harness: 'codex', status: 'working' },
      { id: 'a1', harness: 'codex', status: 'awaiting', awaitingReason: 'question' },
      { id: 'a2', harness: 'claude_code', status: 'awaiting', awaitingReason: 'input' },
      { id: 'i', harness: 'codex', status: 'idle' },
    ]);
    expect(render(list).textContent).toContain('2 waiting');
  });
});
