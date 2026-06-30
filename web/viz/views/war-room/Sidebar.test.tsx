// @vitest-environment jsdom
//
// Sidebar is the toggle roster: a left dock that lists every globe as a scannable
// list — Radar agents (grouped by harness, subagents nested) or Habits orbs (grouped
// by harness, titled). It is presentational; the grouping/nesting is the pure
// `rosterTree`, and clicking a row calls `onPick(id)` which WarRoom turns into the
// existing select → camera-dive → detail-dock flow. Rendered under jsdom.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { Sidebar } from './Sidebar';
import type { HarnessGroup, RosterRow } from '@/viz/modules/radar/rosterTree';

const row = (over: Partial<RosterRow> = {}): RosterRow => ({
  id: 'r',
  title: 'a task',
  subtitle: null,
  harness: 'claude_code',
  depth: 0,
  ...over,
});

const group = (over: Partial<HarnessGroup> = {}): HarnessGroup => ({
  harness: 'claude_code',
  label: 'Claude',
  glyph: '◆',
  color: '#ff8636',
  rows: [],
  ...over,
});

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

const defaults = {
  open: true,
  displayTab: 'radar' as const,
  selectedId: null,
  headerCount: '0 agents',
  onPick: () => {},
  onToggle: () => {},
};

describe('Sidebar', () => {
  it('renders a titled section per harness group with its rows', () => {
    const groups = [
      group({ harness: 'claude_code', label: 'Claude', rows: [row({ id: 'c1' }), row({ id: 'c2' })] }),
      group({ harness: 'codex', label: 'Codex', glyph: '▣', rows: [row({ id: 'x1', harness: 'codex' })] }),
    ];
    const el = render(<Sidebar {...defaults} groups={groups} />);
    expect(el.querySelector('[data-group="claude_code"]')?.textContent).toContain('Claude');
    expect(el.querySelector('[data-group="codex"]')?.textContent).toContain('Codex');
    expect(el.querySelectorAll('[data-roster-id]')).toHaveLength(3);
  });

  it('indents a nested subagent row by its depth', () => {
    const groups = [group({ rows: [row({ id: 'root', depth: 0 }), row({ id: 'kid', depth: 1 })] })];
    const el = render(<Sidebar {...defaults} groups={groups} />);
    expect(el.querySelector('[data-roster-id="root"]')?.getAttribute('data-depth')).toBe('0');
    expect(el.querySelector('[data-roster-id="kid"]')?.getAttribute('data-depth')).toBe('1');
  });

  it('calls onPick with the row id when a row is clicked', () => {
    const onPick = vi.fn();
    const groups = [group({ rows: [row({ id: 'pick-me' })] })];
    const el = render(<Sidebar {...defaults} groups={groups} onPick={onPick} />);
    const btn = el.querySelector('[data-roster-id="pick-me"]') as HTMLElement;
    act(() => (btn.querySelector('button') ?? btn).dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(onPick).toHaveBeenCalledWith('pick-me');
  });

  it('marks the selected row', () => {
    const groups = [group({ rows: [row({ id: 'a' }), row({ id: 'b' })] })];
    const el = render(<Sidebar {...defaults} groups={groups} selectedId="b" />);
    expect(el.querySelector('[data-roster-id="a"]')?.className).not.toContain('is-selected');
    expect(el.querySelector('[data-roster-id="b"]')?.className).toContain('is-selected');
    expect(el.querySelector('[data-roster-id="b"]')?.getAttribute('aria-current')).toBe('true');
  });

  it('reflects the open/closed state via aria-hidden', () => {
    const el = render(<Sidebar {...defaults} groups={[]} open={false} />);
    expect(el.querySelector('.wd-sidebar')?.getAttribute('aria-hidden')).toBe('true');
    act(() => root!.render(<Sidebar {...defaults} groups={[]} open={true} />));
    expect(el.querySelector('.wd-sidebar')?.getAttribute('aria-hidden')).toBe('false');
  });

  it('labels the header per tab and shows the count', () => {
    const radar = render(<Sidebar {...defaults} groups={[]} displayTab="radar" headerCount="12 agents · 3 working" />);
    const head = radar.querySelector('.wd-sidebar-head')?.textContent?.toLowerCase() ?? '';
    expect(head).toContain('roster');
    expect(head).toContain('12 agents');

    act(() => root!.render(<Sidebar {...defaults} groups={[]} displayTab="habits" headerCount="5 habits" />));
    expect(radar.querySelector('.wd-sidebar-head')?.textContent?.toLowerCase()).toContain('habits');
  });

  it('pulses a working radar row but not a closed one', () => {
    const groups = [group({ rows: [row({ id: 'w', status: 'working' }), row({ id: 'c', status: 'closed' })] })];
    const el = render(<Sidebar {...defaults} groups={groups} />);
    expect(el.querySelector('[data-roster-id="w"] .wd-roster-dot')?.className).toContain('is-working');
    expect(el.querySelector('[data-roster-id="c"] .wd-roster-dot')?.className).not.toContain('is-working');
  });
});
