// @vitest-environment jsdom
//
// RadarDetailPanel component tests (Tasks 19–21). Rendered under jsdom with
// react-dom/client + act (house no-deps style; the 3D dive is verified live).
//
// The panel is the click-through readout for one live agent, in four honest
// sections: (1) context gauge + composition, (2) live activity feed, (3) children
// roster, (4) identity + cost. The non-negotiable correctness anchor is HONEST
// COMPOSITION: the exact (API-anchored) bar is ALWAYS shown; the estimated semantic
// bar is shown ONLY when `composition.estimated` is present and is ALWAYS labeled
// "est."; when estimated is null the panel shows "—" and no semantic bar — it must
// never present an estimate as exact, nor fabricate one.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { RadarDetailPanel, relativeTime, uptime } from './RadarDetailPanel';
import type { RadarActivity, RadarAgent, RadarComposition } from './radarTypes';

function agentFixture(over: Partial<RadarAgent> = {}): RadarAgent {
  const composition: RadarComposition = {
    exact: { cacheRead: 120_000, fresh: 40_000, output: 12_000 },
    estimated: { preamble: 8_000, conversation: 90_000, toolOutput: 60_000, thinking: 14_000 },
  };
  return {
    id: 'claude-root',
    harness: 'claude_code',
    origin: 'claude-desktop',
    parentId: null,
    depth: 0,
    label: 'warden',
    nickname: null,
    cwd: 'WARDEN',
    role: null,
    model: 'claude-opus-4-8',
    status: 'working',
    contextTokens: 172_000,
    maxTokens: 200_000,
    fillPct: 0.86,
    contextBreakdown: {
      usedTokens: 172_000,
      maxTokens: 200_000,
      fillPct: 0.86,
      rows: [
        { key: 'messages', label: 'Messages', tokens: 118_000, percent: 0.59, count: null },
        { key: 'skills', label: 'Skills', tokens: 18_000, percent: 0.09, count: null },
        { key: 'mcp_tools', label: 'MCP tools', tokens: 11_000, percent: 0.055, count: 12 },
        { key: 'memory_files', label: 'Memory files', tokens: 4_000, percent: 0.02, count: 3 },
        { key: 'system_prompt', label: 'System prompt', tokens: 3_000, percent: 0.015, count: null },
        { key: 'custom_agents', label: 'Custom agents', tokens: 2_000, percent: 0.01, count: 2 },
        { key: 'free_space', label: 'Free space', tokens: 28_000, percent: 0.14, count: null, muted: true },
      ],
    },
    composition,
    recentActivity: [],
    childCount: 0,
    startedAt: '2026-06-23T22:00:00Z',
    estCostUsd: 0.42,
    ...over,
  };
}

let container: HTMLDivElement | null = null;
let root: Root | null = null;

function render(node: React.ReactNode): HTMLElement {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(node));
  return container;
}

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
});

// ── Task 19: live context window ───────────────────────────────────────────────
describe('RadarDetailPanel — live context window', () => {
  it('renders a static live context-window breakdown with the screenshot-style header and rows', () => {
    const el = render(<RadarDetailPanel agent={agentFixture()} />);
    const section = el.querySelector('[data-context-window]');
    expect(section).toBeTruthy();
    expect((section?.textContent ?? '')).toContain('Context window');
    expect((section?.textContent ?? '')).toContain('172k / 200k (86%)');
    expect((section?.textContent ?? '')).toContain('Messages');
    expect((section?.textContent ?? '')).toContain('118k');
    expect((section?.textContent ?? '')).toContain('59.0%');
    expect((section?.textContent ?? '')).toContain('MCP tools');
    expect((section?.textContent ?? '')).toContain('12');
    expect((section?.textContent ?? '')).toContain('Free space');
    expect((section?.textContent ?? '')).toContain('28k');
    expect(section?.querySelector('button, select, input')).toBeFalsy();
  });

  it('updates the context-window numbers when the selected live agent payload changes', () => {
    const first = agentFixture();
    const second = agentFixture({
      contextTokens: 66_000,
      maxTokens: 100_000,
      fillPct: 0.66,
      contextBreakdown: {
        usedTokens: 66_000,
        maxTokens: 100_000,
        fillPct: 0.66,
        rows: [
          { key: 'messages', label: 'Messages', tokens: 40_000, percent: 0.4, count: null },
          { key: 'free_space', label: 'Free space', tokens: 34_000, percent: 0.34, count: null, muted: true },
        ],
      },
    });

    const el = render(<RadarDetailPanel agent={first} />);
    expect((el.querySelector('[data-context-window]')?.textContent ?? '')).toContain('172k / 200k (86%)');

    act(() => root!.render(<RadarDetailPanel agent={second} />));

    const section = el.querySelector('[data-context-window]');
    expect((section?.textContent ?? '')).toContain('66k / 100k (66%)');
    expect((section?.textContent ?? '')).toContain('34k');
    expect((section?.textContent ?? '')).not.toContain('172k / 200k');
  });

  it('falls back to occupancy and free-space rows when no breakdown is present', () => {
    const el = render(
      <RadarDetailPanel
        agent={{
          ...agentFixture({ fillPct: 0.25, contextTokens: 25_000, maxTokens: 100_000 }),
          contextBreakdown: undefined,
        }}
      />,
    );
    const section = el.querySelector('[data-context-window]');
    expect((section?.textContent ?? '')).toContain('25k / 100k (25%)');
    expect((section?.textContent ?? '')).toContain('Context');
    expect((section?.textContent ?? '')).toContain('Free space');
  });

  it('treats an empty normalized breakdown as absent for both header and rows', () => {
    const el = render(
      <RadarDetailPanel
        agent={{
          ...agentFixture({ fillPct: 0.94, contextTokens: 188_000, maxTokens: 200_000 }),
          contextBreakdown: { usedTokens: 0, maxTokens: 0, fillPct: 0, rows: [] },
        }}
      />,
    );
    const section = el.querySelector('[data-context-window]');
    expect((section?.textContent ?? '')).toContain('188k / 200k (94%)');
    expect((section?.textContent ?? '')).toContain('Free space');
    expect((section?.textContent ?? '')).not.toContain('0 / ∞');
  });
});

// ── Task 20: live activity feed ───────────────────────────────────────────────
describe('RadarDetailPanel — live activity feed', () => {
  const activity = (over: Partial<RadarActivity>): RadarActivity => ({
    ts: '2026-06-23T22:00:00Z',
    kind: 'message',
    label: 'untitled',
    ...over,
  });

  it('renders recentActivity newest-first with kind labels', () => {
    const recent: RadarActivity[] = [
      activity({ ts: '2026-06-23T22:00:00Z', kind: 'message', label: 'oldest msg' }),
      activity({ ts: '2026-06-23T22:01:00Z', kind: 'thinking', label: 'mid think' }),
      activity({ ts: '2026-06-23T22:02:00Z', kind: 'tool', label: 'newest tool' }),
    ];
    const el = render(<RadarDetailPanel agent={agentFixture({ recentActivity: recent })} />);
    const rows = Array.from(el.querySelectorAll('[data-activity-row]'));
    expect(rows).toHaveLength(3);
    // newest first → the tool call leads, the oldest message trails.
    expect(rows[0].textContent).toContain('newest tool');
    expect(rows[2].textContent).toContain('oldest msg');
    // kind is surfaced (label/title), not only colour.
    const feed = el.querySelector('[data-section="activity"]');
    expect((feed?.textContent ?? '').toLowerCase()).toContain('tool');
    expect((feed?.textContent ?? '').toLowerCase()).toContain('thinking');
  });

  it('handles the empty feed with an explicit empty state', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ recentActivity: [] })} />);
    const feed = el.querySelector('[data-section="activity"]');
    expect(feed).toBeTruthy();
    expect(el.querySelectorAll('[data-activity-row]')).toHaveLength(0);
    expect((feed?.textContent ?? '').toLowerCase()).toMatch(/no (recent )?activity|quiet|idle/);
  });
});

describe('relativeTime — honest, tolerant', () => {
  const now = Date.UTC(2026, 5, 23, 22, 5, 0); // 2026-06-23T22:05:00Z
  it('formats sub-minute / minute / hour deltas', () => {
    expect(relativeTime('2026-06-23T22:04:30Z', now)).toMatch(/s ago|just now/);
    expect(relativeTime('2026-06-23T22:00:00Z', now)).toBe('5m ago');
    expect(relativeTime('2026-06-23T20:05:00Z', now)).toBe('2h ago');
  });
  it('returns an empty string for an unparseable timestamp (never NaN)', () => {
    expect(relativeTime('not-a-date', now)).toBe('');
    expect(relativeTime('', now)).toBe('');
  });
});

describe('uptime — duration since startedAt', () => {
  const now = Date.UTC(2026, 5, 23, 22, 5, 0);
  it('formats a running duration compactly', () => {
    expect(uptime('2026-06-23T22:00:00Z', now)).toBe('5m');
    expect(uptime('2026-06-23T20:05:00Z', now)).toBe('2h 0m');
  });
  it('returns a dash for a missing/unparseable start (never NaN)', () => {
    expect(uptime('', now)).toBe('—');
    expect(uptime('nope', now)).toBe('—');
  });
});

// ── Task 21: children roster + identity / cost ────────────────────────────────
describe('RadarDetailPanel — children roster + identity/cost', () => {
  const child = (over: Partial<RadarAgent>): RadarAgent =>
    agentFixture({
      id: 'child',
      parentId: 'claude-root',
      depth: 1,
      childCount: 0,
      role: 'explorer',
      nickname: null,
      ...over,
    });

  it('lists each passed-in child with status + fill %, and jumps on click', () => {
    const children = [
      child({ id: 'kid-a', role: 'explorer', fillPct: 0.3, status: 'working' }),
      child({ id: 'kid-b', nickname: 'Bohr', role: null, fillPct: 0.55, status: 'idle' }),
    ];
    const onJumpTo = vi.fn();
    const el = render(
      <RadarDetailPanel agent={agentFixture({ childCount: 2 })} children={children} onJumpTo={onJumpTo} />,
    );
    const rows = Array.from(el.querySelectorAll('[data-roster-row]')) as HTMLElement[];
    expect(rows).toHaveLength(2);
    expect(rows[0].textContent).toContain('explorer');
    expect(rows[0].textContent).toContain('30%');
    expect(rows[1].textContent).toContain('Bohr'); // nickname when role is null
    expect(rows[1].textContent).toContain('55%');

    act(() => {
      (rows[1].querySelector('button') ?? rows[1]).dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(onJumpTo).toHaveBeenCalledWith('kid-b');
  });

  it('omits the roster for a flat agent (no fabricated children)', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ childCount: 0 })} children={[]} />);
    expect(el.querySelectorAll('[data-roster-row]')).toHaveLength(0);
    // no empty "Children" section rendered when there are genuinely none.
    expect(el.querySelector('[data-section="roster"]')).toBeFalsy();
  });

  it('shows identity (model) and a formatted cost, with a dash when cost is null', () => {
    const withCost = render(<RadarDetailPanel agent={agentFixture({ estCostUsd: 0.42 })} />);
    const id1 = withCost.querySelector('[data-section="identity"]');
    expect((id1?.textContent ?? '')).toContain('claude-opus-4-8');
    expect((id1?.textContent ?? '')).toContain('$0.42');

    act(() => root?.unmount());
    container?.remove();
    root = null;
    container = null;

    const noCost = render(<RadarDetailPanel agent={agentFixture({ estCostUsd: null })} />);
    const id2 = noCost.querySelector('[data-section="identity"]');
    // cost cell shows a dash, never a fabricated number.
    const costCell = id2?.querySelector('[data-id="cost"]');
    expect((costCell?.textContent ?? '')).toContain('—');
    expect((costCell?.textContent ?? '')).not.toContain('$');
  });
});
