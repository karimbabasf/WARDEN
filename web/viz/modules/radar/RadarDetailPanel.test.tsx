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
// so a test never touches the filesystem. Commands a test cares about are asserted
// explicitly through `invoke`; everything else gets an inert reply.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(() => Promise.resolve(undefined)),
}));

// No Tauri event bus in jsdom: hand any subscriber an unsubscribe and never fire.
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
    // The panel still stands up and still renders its first real section, rather
    // than throwing on a field an older backend never sent.
    expect(el.querySelector('[data-context-window]')).toBeTruthy();
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
   * command on its default reply. Dispatching by command name rather than with
   * `mockResolvedValueOnce` keeps the stub order-independent, so another mount-time
   * IPC call can never consume the reply this test set up.
   */
  function onRename(result: unknown, rejects = false) {
    vi.mocked(invoke).mockImplementation(((cmd: string) => {
      if (cmd === 'rename_session') {
        return rejects ? Promise.reject(result) : Promise.resolve(result);
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
  // IPC calls, so a blanket "never called" would be asserting something this test
  // never meant.
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

// ── "Take me there": raise the agent's terminal window ─────────────────────────
//
// The rule under test is the one the whole control exists for: the button renders
// only for a window the backend already said it can raise. Every other case shows
// the backend's own reason instead, because an IDE-hosted session, a closed window
// and an unsupported emulator are three different facts, not one dead button.
describe('RadarDetailPanel — take me there', () => {
  type Target = {
    reachable: boolean;
    app: string | null;
    viaAgentId: string | null;
    viaLabel: string | null;
    reason: string | null;
  };

  const reachable = (over: Partial<Target> = {}): Target => ({
    reachable: true,
    app: 'Terminal',
    viaAgentId: null,
    viaLabel: null,
    reason: null,
    ...over,
  });

  /** Route the two terminal commands by name; everything else keeps its default. */
  function onTerminal(target: unknown, focus: unknown = { ok: true, denied: false, message: null }) {
    vi.mocked(invoke).mockImplementation(((cmd: string) => {
      if (cmd === 'agent_terminal_target') return Promise.resolve(target);
      if (cmd === 'focus_agent_terminal') return Promise.resolve(focus);
      return Promise.resolve(undefined);
    }) as never);
  }

  async function mount(target: unknown, focus?: unknown) {
    onTerminal(target, focus);
    const el = render(<RadarDetailPanel agent={agentFixture({ id: 'a1' })} />);
    await act(async () => {});
    return el;
  }

  it('offers the button and names the app when the backend resolved a window', async () => {
    const el = await mount(reachable());
    const btn = el.querySelector<HTMLButtonElement>('[data-goto="button"]');
    expect(btn?.textContent).toContain('Take me there');
    expect(el.querySelector('.wd-detail-goto-app')?.textContent).toBe('Terminal');

    await act(async () => {
      btn!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(invoke).toHaveBeenCalledWith('focus_agent_terminal', { agentId: 'a1' });
  });

  it('states the reason instead of a disabled button when there is no window to raise', async () => {
    const el = await mount({
      reachable: false,
      app: null,
      viaAgentId: null,
      viaLabel: null,
      reason: 'this session runs inside Visual Studio Code and has no terminal window',
    });
    expect(el.querySelector('[data-goto="button"]')).toBeFalsy();
    expect(el.querySelector('[data-goto="blocked"]')?.textContent).toContain('Visual Studio Code');
  });

  it("names the root whose window it is about to raise, for a subagent", async () => {
    // A subagent has no process of its own: the window belongs to its root, and
    // jumping there silently would land the operator somewhere they did not select.
    const el = await mount(reachable({ viaAgentId: 'root-1', viaLabel: 'warden' }));
    expect(el.querySelector('.wd-detail-goto-app')?.textContent).toBe('Terminal · in warden');
    expect(el.querySelector('[data-goto="button"]')?.getAttribute('aria-label')).toBe(
      'Take me there: the Terminal window running warden',
    );
  });

  it('offers System Settings only for the refusal that never re-prompts', async () => {
    const el = await mount(reachable(), {
      ok: false,
      denied: true,
      message: 'macOS blocked the request',
    });
    await act(async () => {
      el.querySelector<HTMLButtonElement>('[data-goto="button"]')!
        .dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    const settings = el.querySelector<HTMLButtonElement>('.wd-detail-goto-settings');
    expect(el.querySelector('.wd-detail-goto-error')?.textContent).toContain('macOS blocked');
    await act(async () => {
      settings!.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(invoke).toHaveBeenCalledWith('open_automation_settings');
  });

  it('reports an ordinary failure without offering a settings pane that would not help', async () => {
    const el = await mount(reachable(), {
      ok: false,
      denied: false,
      message: 'that window has closed since WARDEN last looked',
    });
    await act(async () => {
      el.querySelector<HTMLButtonElement>('[data-goto="button"]')!
        .dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(el.querySelector('.wd-detail-goto-error')?.textContent).toContain('has closed');
    expect(el.querySelector('.wd-detail-goto-settings')).toBeFalsy();
  });

  it('renders nothing at all when there is no backend to ask (the browser sandbox)', async () => {
    vi.mocked(invoke).mockImplementation((() => Promise.reject('no tauri bridge')) as never);
    const el = render(<RadarDetailPanel agent={agentFixture({ id: 'a1' })} />);
    await act(async () => {});
    expect(el.querySelector('[data-goto="button"]')).toBeFalsy();
    expect(el.querySelector('[data-goto="blocked"]')).toBeFalsy();
  });

  it('re-probes on a new agent and ignores the previous agent\'s late answer', async () => {
    // Clicking quickly through the board must never leave agent B showing A's window.
    const replies: Array<(v: unknown) => void> = [];
    vi.mocked(invoke).mockImplementation(((cmd: string) => {
      if (cmd === 'agent_terminal_target') return new Promise((res) => replies.push(res));
      return Promise.resolve(undefined);
    }) as never);

    const el = render(<RadarDetailPanel agent={agentFixture({ id: 'a1' })} />);
    act(() => {
      root!.render(<RadarDetailPanel agent={agentFixture({ id: 'a2' })} />);
    });
    expect(replies).toHaveLength(2);

    // The SECOND agent answers first, then the first agent's stale reply lands.
    await act(async () => {
      replies[1](reachable({ app: 'iTerm2' }));
    });
    await act(async () => {
      replies[0](reachable({ app: 'Terminal' }));
    });
    expect(el.querySelector('.wd-detail-goto-app')?.textContent).toBe('iTerm2');
  });

  // ── the panel that stopped jumping ───────────────────────────────────────────
  //
  // The probe is a round trip, so between two agents this control was empty for a
  // frame or two and everything below it (the context gauge, the activity list, the
  // roster) hopped 37px each time. The claim is still cleared the instant the panel
  // re-points; only the SPACE is held, and only when a row was actually there before.

  it('holds its height while re-probing, so the readouts below do not hop', async () => {
    const replies: Array<(v: unknown) => void> = [];
    vi.mocked(invoke).mockImplementation(((cmd: string) => {
      if (cmd === 'agent_terminal_target') return new Promise((res) => replies.push(res));
      return Promise.resolve(undefined);
    }) as never);

    const el = render(<RadarDetailPanel agent={agentFixture({ id: 'a1' })} />);
    await act(async () => {
      replies[0](reachable({ app: 'Terminal' }));
    });
    expect(el.querySelector('[data-goto="button"]')).toBeTruthy();

    // Re-point at another agent. The answer has not come back yet.
    act(() => {
      root!.render(<RadarDetailPanel agent={agentFixture({ id: 'a2' })} />);
    });
    // The stale claim is gone...
    expect(el.querySelector('[data-goto="button"]')).toBeFalsy();
    expect(el.querySelector('.wd-detail-goto-app')).toBeFalsy();
    // ...and the row it lived in is still occupying its place.
    const held = el.querySelector('.wd-detail-goto-held');
    expect(held).toBeTruthy();
    expect(held?.getAttribute('aria-hidden')).toBe('true');

    await act(async () => {
      replies[1](reachable({ app: 'iTerm2' }));
    });
    expect(el.querySelector('.wd-detail-goto-held')).toBeFalsy();
    expect(el.querySelector('.wd-detail-goto-app')?.textContent).toBe('iTerm2');
  });

  it('reserves nothing for an agent that never had a row', async () => {
    // Holding space for something that is never coming is its own layout bug, and in
    // the browser sandboxes (no bridge at all) it would be permanent.
    vi.mocked(invoke).mockImplementation((() => Promise.reject('no tauri bridge')) as never);
    const el = render(<RadarDetailPanel agent={agentFixture({ id: 'a1' })} />);
    await act(async () => {});
    act(() => {
      root!.render(<RadarDetailPanel agent={agentFixture({ id: 'a2' })} />);
    });
    expect(el.querySelector('.wd-detail-goto-held')).toBeFalsy();
  });

  it('marks the button busy while the raise is in flight and swallows a second click', async () => {
    // Raising a window is an AppleScript round trip through another process. Without
    // a pending state a slow raise reads as a dead button and gets clicked again.
    const raises: Array<(v: unknown) => void> = [];
    vi.mocked(invoke).mockImplementation(((cmd: string) => {
      if (cmd === 'agent_terminal_target') return Promise.resolve(reachable());
      if (cmd === 'focus_agent_terminal') return new Promise((res) => raises.push(res));
      return Promise.resolve(undefined);
    }) as never);

    const el = render(<RadarDetailPanel agent={agentFixture({ id: 'a1' })} />);
    await act(async () => {});
    const btn = el.querySelector<HTMLButtonElement>('[data-goto="button"]')!;

    await act(async () => {
      btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(btn.getAttribute('aria-busy')).toBe('true');

    await act(async () => {
      btn.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });
    expect(raises).toHaveLength(1);

    await act(async () => {
      raises[0]({ ok: true, denied: false, message: null });
    });
    expect(el.querySelector('[data-goto="button"]')?.getAttribute('aria-busy')).toBeNull();
  });
});
