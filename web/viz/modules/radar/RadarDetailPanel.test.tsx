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
import { invoke } from '@tauri-apps/api/core';
import { RadarDetailPanel, relativeTime, uptime } from './RadarDetailPanel';
import type {
  RadarActivity,
  RadarAgent,
  RadarComposition,
  RadarCurrentAction,
  RadarTeam,
} from '@/viz/shared/types/radarTypes';

// reveal_path / preview_file / rename_session are real Tauri IPC calls; stub them
// so a test never touches the filesystem.
//
// The mock DISPATCHES ON COMMAND NAME rather than returning one value for
// everything. The panel mounts a CompactControl, which reads `compact_status` on
// mount, and a blanket stub made that call resolve to `undefined`, which then
// satisfied assertions meant for `rename_session` and made the rename tests fail
// against a value they never asked for. Commands a test cares about are asserted
// explicitly through `invoke`; everything else gets an inert, well-shaped reply.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn((cmd: string) => {
    if (cmd === 'compact_status') {
      return Promise.resolve({ automation: 'unknown', automationRecoverableInSettings: false, armed: [] });
    }
    return Promise.resolve(undefined);
  }),
}));

// CompactControl subscribes to the backend's `compact_status` push. There is no
// Tauri event bus in jsdom, so hand it an unsubscribe and never fire.
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

function agentFixture(over: Partial<RadarAgent> = {}): RadarAgent {
  const composition: RadarComposition = {
    exact: { cacheRead: 120_000, fresh: 40_000, cacheWrite: 0, output: 12_000 },
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
    repo: null,
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
  // `mockReset`, not `mockClear`: a test that installs its own implementation via
  // `onRename` would otherwise leak that implementation into every test after it.
  // Reset restores the command-dispatching default from the `vi.mock` factory.
  vi.mocked(invoke).mockReset();
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
  });

  // The header used to be an inert div that still rendered a caret glyph, so it
  // advertised a disclosure that did not exist. It is a real one now: the exact
  // API-anchored token split is worth having but not worth four permanent rows of
  // a narrow rail, so it lives behind the toggle.
  it('hides the exact token split behind a real disclosure on the header', () => {
    const el = render(<RadarDetailPanel agent={agentFixture()} />);
    const head = el.querySelector('.wd-context-head') as HTMLButtonElement;
    expect(head).toBeTruthy();
    expect(head.tagName).toBe('BUTTON');
    expect(head.getAttribute('aria-expanded')).toBe('false');
    expect(el.querySelector('.wd-context-exact')).toBeFalsy();

    act(() => head.dispatchEvent(new MouseEvent('click', { bubbles: true })));

    expect(head.getAttribute('aria-expanded')).toBe('true');
    const exact = el.querySelector('.wd-context-exact');
    expect(exact).toBeTruthy();
    expect((exact?.textContent ?? '')).toContain('Cache read');
    expect((exact?.textContent ?? '')).toContain('Output');
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

// ── current-action hero ─────────────────────────────────────────────────────────
// The hero used to be CURRENT ACTION: the in-flight call and nothing else, which
// left the panel's first section blank for every idle agent. It is AGENT SUMMARY
// now. The live call is still its headline when there is one (every assertion about
// that behaviour below is unchanged apart from the section name), and what the agent
// has actually done is the part that is new.
describe('RadarDetailPanel — agent summary hero', () => {
  const HERO = '[data-section="agent-summary"]';
  const action = (over: Partial<RadarCurrentAction> = {}): RadarCurrentAction => ({
    kind: 'write',
    tool: 'Edit',
    label: 'Edit agent.rs',
    target: '~/Developer/Apps/WARDEN/src-tauri/src/radar/agent.rs',
    startedAt: '2026-06-23T22:00:00Z',
    elapsedMs: 4_200,
    ...over,
  });

  it('leads with the in-flight action, above the context window', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ currentAction: action() })} />);
    const hero = el.querySelector(HERO);
    expect(hero).toBeTruthy();
    expect(hero?.getAttribute('data-current-action')).toBe('active');
    expect(hero?.getAttribute('data-kind')).toBe('write');
    expect((hero?.textContent ?? '')).toContain('Editing agent.rs');
    expect((hero?.textContent ?? '')).toContain('Edit');
    // it sits before the context window in DOM order, i.e. it IS the first thing
    // under the header.
    const sections = Array.from(el.querySelectorAll('.wd-radar-section'));
    expect(sections[0]).toBe(hero);
    expect(sections[1]?.hasAttribute('data-context-window')).toBe(true);
  });

  // The whole point of the swap: with nothing in flight the section still answers
  // "what has this agent been doing" instead of only "nothing right now".
  it('says what the agent last did, and counts what it has done, when nothing is in flight', () => {
    const recent: RadarActivity[] = [
      { ts: '2026-06-23T22:00:00Z', kind: 'write', label: 'Edit agent.rs', target: '~/WARDEN/agent.rs' },
      { ts: '2026-06-23T21:59:00Z', kind: 'read', label: 'Read radar.rs', target: '~/WARDEN/radar.rs' },
      { ts: '2026-06-23T21:58:00Z', kind: 'read', label: 'Read store.rs', target: '~/WARDEN/store.rs' },
    ];
    const el = render(
      <RadarDetailPanel agent={agentFixture({ currentAction: null, recentActivity: recent })} />,
    );
    const hero = el.querySelector(HERO);
    expect(hero?.getAttribute('data-current-action')).toBe('idle');
    expect((hero?.textContent ?? '')).toContain('Last edited agent.rs');
    // Exact counts off the real feed, never a rounded or invented figure.
    const tally = (hero?.querySelector('[data-summary-tally]')?.textContent ?? '');
    expect(tally).toContain('2 files read');
    expect(tally).toContain('1 edit');
    // And which files, because a count on its own is half an answer.
    const files = hero?.querySelector('[data-summary-files]')?.textContent ?? '';
    expect(files).toContain('agent.rs');
    expect(files).toContain('store.rs');
  });

  it('says the feed is empty rather than inventing a summary', () => {
    const el = render(
      <RadarDetailPanel agent={agentFixture({ currentAction: null, recentActivity: [] })} />,
    );
    const hero = el.querySelector(HERO);
    expect((hero?.textContent ?? '')).toContain('No recorded actions yet');
    expect(hero?.querySelector('[data-summary-tally]')).toBeFalsy();
    expect(hero?.querySelector('[data-summary-files]')).toBeFalsy();
  });

  // The hero offers TWO distinct verbs on a target, so each is asserted by its
  // own accessible name rather than by "the first button in the section".
  it('shows a clickable, accessibly-named Finder-reveal button when a target is present, and calls reveal_path with it', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ currentAction: action() })} />);
    const btn = el.querySelector(
      `${HERO} button[aria-label="Reveal agent.rs in Finder"]`,
    ) as HTMLButtonElement;
    expect(btn).toBeTruthy();
    act(() => btn.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(invoke).toHaveBeenCalledWith('reveal_path', { path: '~/Developer/Apps/WARDEN/src-tauri/src/radar/agent.rs' });
  });

  it('offers an in-app view toggle for the in-flight target, separate from Finder', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ currentAction: action() })} />);
    const view = el.querySelector(`${HERO} .wd-action-hero-target`) as HTMLButtonElement;
    expect(view).toBeTruthy();
    expect(view.getAttribute('aria-expanded')).toBe('false');
    // Shows the path with the FILENAME intact: the directory is the part allowed
    // to truncate, because the filename is what you actually needed.
    expect((view.textContent ?? '')).toContain('agent.rs');

    act(() => view.dispatchEvent(new MouseEvent('click', { bubbles: true })));

    expect(view.getAttribute('aria-expanded')).toBe('true');
    expect(el.querySelector('[data-file-preview]')).toBeTruthy();
    expect(invoke).toHaveBeenCalledWith('preview_file', {
      path: '~/Developer/Apps/WARDEN/src-tauri/src/radar/agent.rs',
    });
  });

  it('renders no target controls when the action has no single-file target (e.g. a shell run)', () => {
    const el = render(
      <RadarDetailPanel
        agent={agentFixture({
          currentAction: action({ kind: 'run', tool: 'Bash', label: 'pnpm test', target: null }),
          recentActivity: [],
        })}
      />,
    );
    const hero = el.querySelector(HERO);
    expect(hero?.querySelector('.wd-action-hero-target')).toBeFalsy();
    expect(hero?.querySelector('[aria-label^="Reveal"]')).toBeFalsy();
    expect((hero?.textContent ?? '')).toContain('pnpm test');
  });

  it('ticks the elapsed clock live while an action is in flight', () => {
    vi.useFakeTimers();
    try {
      const start = new Date('2026-06-23T22:00:00.000Z');
      vi.setSystemTime(start);
      const el = render(
        <RadarDetailPanel agent={agentFixture({ currentAction: action({ startedAt: start.toISOString() }) })} />,
      );
      const value = () => el.querySelector('.wd-action-hero-elapsed-value')?.textContent;
      expect(value()).toBe('0s');
      act(() => {
        vi.advanceTimersByTime(3_000);
      });
      expect(value()).toBe('3s');
    } finally {
      vi.useRealTimers();
    }
  });

  it('falls back to the backend elapsedMs when startedAt cannot be parsed', () => {
    const el = render(
      <RadarDetailPanel agent={agentFixture({ currentAction: action({ startedAt: 'not-a-date', elapsedMs: 42_000 }) })} />,
    );
    expect(el.querySelector('.wd-action-hero-elapsed-value')?.textContent).toBe('42s');
  });

  it('swallows a rejected reveal_path without throwing', async () => {
    vi.mocked(invoke).mockRejectedValueOnce(new Error('no such path'));
    const errSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    const el = render(<RadarDetailPanel agent={agentFixture({ currentAction: action() })} />);
    const btn = el.querySelector(
      `${HERO} button[aria-label="Reveal agent.rs in Finder"]`,
    ) as HTMLButtonElement;
    await act(async () => {
      btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(errSpy).toHaveBeenCalled();
    errSpy.mockRestore();
  });
});

// ── activity feed Finder reveal ──────────────────────────────────────────────────
describe('RadarDetailPanel — activity feed Finder reveal', () => {
  // A feed row is a disclosure now: collapsed it is one skimmable line, expanded
  // it gives the untruncated label and the file verbs. So the file controls are
  // asserted AFTER expanding, which is the real user path.
  it('reveals a row with a target and calls reveal_path with its path', () => {
    const recent: RadarActivity[] = [
      { ts: '2026-06-23T22:00:00Z', kind: 'read', label: 'Read agent.rs', target: '~/WARDEN/agent.rs' },
    ];
    const el = render(<RadarDetailPanel agent={agentFixture({ recentActivity: recent })} />);
    const row = el.querySelector('[data-activity-row]') as HTMLElement;
    const toggle = row.querySelector('.wd-radar-feed-main') as HTMLButtonElement;
    expect(toggle.getAttribute('aria-expanded')).toBe('false');

    act(() => toggle.dispatchEvent(new MouseEvent('click', { bubbles: true })));

    const btn = row.querySelector('button[aria-label="Reveal agent.rs in Finder"]') as HTMLButtonElement;
    expect(btn).toBeTruthy();
    act(() => btn.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(invoke).toHaveBeenCalledWith('reveal_path', { path: '~/WARDEN/agent.rs' });
  });

  it('does not render a reveal control on a row with no target', () => {
    const recent: RadarActivity[] = [{ ts: '2026-06-23T22:00:00Z', kind: 'run', label: 'pnpm test' }];
    const el = render(<RadarDetailPanel agent={agentFixture({ recentActivity: recent })} />);
    const row = el.querySelector('[data-activity-row]') as HTMLElement;
    const toggle = row.querySelector('.wd-radar-feed-main') as HTMLButtonElement;

    act(() => toggle.dispatchEvent(new MouseEvent('click', { bubbles: true })));

    // Expanded, and still no file verbs: there is no single file for a shell run,
    // so the row says so rather than offering a control that cannot work.
    expect(row.querySelector('[aria-label^="Reveal"]')).toBeFalsy();
    expect((row.textContent ?? '')).toContain('No single file for this row');
  });

  it('keeps only one row open at a time so the feed cannot accordion', () => {
    const recent: RadarActivity[] = [
      { ts: '2026-06-23T22:00:00Z', kind: 'read', label: 'Read a.rs', target: '~/WARDEN/a.rs' },
      { ts: '2026-06-23T21:00:00Z', kind: 'read', label: 'Read b.rs', target: '~/WARDEN/b.rs' },
    ];
    const el = render(<RadarDetailPanel agent={agentFixture({ recentActivity: recent })} />);
    const toggles = el.querySelectorAll('.wd-radar-feed-main');

    act(() => toggles[0].dispatchEvent(new MouseEvent('click', { bubbles: true })));
    act(() => toggles[1].dispatchEvent(new MouseEvent('click', { bubbles: true })));

    expect(toggles[0].getAttribute('aria-expanded')).toBe('false');
    expect(toggles[1].getAttribute('aria-expanded')).toBe('true');
  });
});

// ── session title + team membership ──────────────────────────────────────────────
describe('RadarDetailPanel — session title + team membership', () => {
  const team = (over: Partial<RadarTeam> = {}): RadarTeam => ({
    id: 'session-f3e4ef77',
    name: 'session-f3e4ef77',
    memberName: 'BackendMap',
    memberType: 'Explore',
    memberCount: 6,
    isLead: false,
    ...over,
  });

  it("shows the harness's own session title in the header, distinct from the derived heading", () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ title: 'Rebuild the radar panel', label: 'warden' })} />);
    expect(el.querySelector('.wd-detail-title')?.textContent).toBe('warden');
    expect(el.querySelector('.wd-detail-session-name')?.textContent).toBe('Rebuild the radar panel');
  });

  it('omits the session-title line when the harness never named the session', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ title: null })} />);
    expect(el.querySelector('.wd-detail-session-name')).toBeFalsy();
  });

  it('shows team name, member type, and lead status in Identity', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ team: team({ isLead: true }) })} />);
    const id = el.querySelector('[data-id="team"]');
    expect((id?.textContent ?? '')).toContain('session-f3e4ef77');
    expect((id?.textContent ?? '')).toContain('Explore');
    expect((id?.textContent ?? '')).toContain('lead');
    expect(el.querySelector('[data-id="team-member"]')?.textContent).toContain('BackendMap');
  });

  it('omits the team rows when the agent has no team', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ team: null })} />);
    expect(el.querySelector('[data-id="team"]')).toBeFalsy();
    expect(el.querySelector('[data-id="team-member"]')).toBeFalsy();
  });
});

// ── safe degradation: older backend omitting the new fields entirely ────────────
describe('RadarDetailPanel — safe degradation without the new fields', () => {
  it('renders without throwing when title/currentAction/team are absent from the agent object', () => {
    const full = agentFixture();
    const { title, currentAction, team, ...bare } = full;
    void title;
    void currentAction;
    void team;
    const el = render(<RadarDetailPanel agent={bare as RadarAgent} />);
    expect(el.querySelector('[data-current-action="idle"]')).toBeTruthy();
    expect(el.querySelector('.wd-detail-session-name')).toBeFalsy();
    expect(el.querySelector('[data-id="team"]')).toBeFalsy();
  });
});

// Renaming previously meant hand-editing a transcript, so these paths are the feature,
// not a convenience. The subtle one is the poll guard: the radar refreshes on an
// interval and must never overwrite a name while it is being typed.
describe('RadarDetailPanel rename', () => {
  function openEditor(el: HTMLElement): HTMLInputElement {
    const pencil = el.querySelector<HTMLButtonElement>('.wd-detail-rename');
    if (!pencil) throw new Error('rename control missing');
    act(() => pencil.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    const input = el.querySelector<HTMLInputElement>('.wd-detail-title-input');
    if (!input) throw new Error('rename input did not open');
    return input;
  }

  function type(input: HTMLInputElement, value: string) {
    const setter = Object.getOwnPropertyDescriptor(
      window.HTMLInputElement.prototype,
      'value',
    )?.set;
    act(() => {
      setter?.call(input, value);
      input.dispatchEvent(new Event('input', { bubbles: true }));
    });
  }

  function press(input: HTMLInputElement, key: string) {
    act(() => {
      input.dispatchEvent(new KeyboardEvent('keydown', { key, bubbles: true }));
    });
  }

  it('exposes an accessibly-named rename control', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ label: 'warden' })} />);
    const pencil = el.querySelector<HTMLButtonElement>('.wd-detail-rename');
    expect(pencil).not.toBeNull();
    expect(pencil?.getAttribute('aria-label')).toBe('Rename warden');
  });

  /**
   * Set what `rename_session` resolves (or rejects) with, leaving every other
   * command on its default reply.
   *
   * `mockResolvedValueOnce` cannot be used here any more: the panel mounts a
   * compaction control that reads `compact_status`, and that call would consume
   * the one-shot before the rename ever ran. Dispatching by command name is
   * order-independent, which is the property the test actually needs.
   */
  function onRename(result: unknown, rejects = false) {
    vi.mocked(invoke).mockImplementation(((cmd: string) => {
      if (cmd === 'rename_session') {
        return rejects ? Promise.reject(result) : Promise.resolve(result);
      }
      if (cmd === 'compact_status') {
        return Promise.resolve({ automation: 'unknown', automationRecoverableInSettings: false, armed: [] });
      }
      return Promise.resolve(undefined);
    }) as never);
  }

  it('commits on Enter, invoking rename_session with the trimmed name', async () => {
    onRename('renamed board');
    const el = render(<RadarDetailPanel agent={agentFixture({ id: 'a1', label: 'warden' })} />);
    const input = openEditor(el);
    type(input, '  renamed board  ');
    press(input, 'Enter');
    await act(async () => {});

    expect(invoke).toHaveBeenCalledWith('rename_session', {
      agentId: 'a1',
      name: 'renamed board',
    });
    expect(el.querySelector('.wd-detail-title')?.textContent).toBe('renamed board');
  });

  it('shows the name Rust returned, not the raw typed string', async () => {
    // The backend trims and truncates, so its answer is the source of truth.
    onRename('cleaned-by-rust');
    const el = render(<RadarDetailPanel agent={agentFixture({ label: 'warden' })} />);
    const input = openEditor(el);
    type(input, 'whatever the user typed');
    press(input, 'Enter');
    await act(async () => {});
    expect(el.querySelector('.wd-detail-title')?.textContent).toBe('cleaned-by-rust');
  });

  it('Escape cancels without invoking a rename', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ label: 'warden' })} />);
    const input = openEditor(el);
    type(input, 'discard me');
    press(input, 'Escape');
    const renames = (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls.filter(
      (c) => c[0] === 'rename_session',
    );
    expect(renames).toHaveLength(0);
    expect(el.querySelector('.wd-detail-title')?.textContent).toBe('warden');
  });

  it('restores the previous name and surfaces the error when the backend rejects', async () => {
    onRename('name contains control characters', true);
    const el = render(<RadarDetailPanel agent={agentFixture({ label: 'warden' })} />);
    const input = openEditor(el);
    type(input, 'badname');
    press(input, 'Enter');
    await act(async () => {});

    expect(el.querySelector('.wd-detail-title')?.textContent).toBe('warden');
    expect(el.querySelector('[role="alert"]')?.textContent).toContain('control characters');
  });

  // Asserts specifically that no RENAME is sent. The panel legitimately makes other
  // IPC calls on mount now (the compaction control reads its status), so a blanket
  // "never called" would be asserting something this test never meant.
  it('does not invoke a rename when the name is unchanged or blank', () => {
    const renames = () =>
      (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls.filter(
        (c) => c[0] === 'rename_session',
      ).length;

    const el = render(<RadarDetailPanel agent={agentFixture({ label: 'warden' })} />);
    const input = openEditor(el);
    press(input, 'Enter');
    expect(renames()).toBe(0);

    const again = openEditor(el);
    type(again, '   ');
    press(again, 'Enter');
    expect(renames()).toBe(0);
  });

  it('a radar refresh arriving mid-edit does not clobber the in-progress text', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ id: 'a1', label: 'warden' })} />);
    const input = openEditor(el);
    type(input, 'half-typed nam');

    // The radar polls on an interval; a fresh frame lands while the user is typing.
    act(() => {
      root!.render(
        <RadarDetailPanel agent={agentFixture({ id: 'a1', label: 'server-renamed' })} />,
      );
    });

    const still = el.querySelector<HTMLInputElement>('.wd-detail-title-input');
    expect(still?.value).toBe('half-typed nam');
  });

  it('renders no rename affordance for an agent whose name cannot be resolved to an id', () => {
    const el = render(<RadarDetailPanel agent={agentFixture({ id: '', label: 'warden' })} />);
    expect(el.querySelector('.wd-detail-rename')).toBeFalsy();
    expect(el.querySelector('.wd-detail-title')?.textContent).toBe('warden');
  });
});
