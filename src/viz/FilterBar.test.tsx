// @vitest-environment jsdom
//
// FilterBar is the severity + harness emphasis filter, lifted out of chrome.tsx
// into its own bottom-centre dock (the StatusDeck it sat above is removed). The
// behaviour is unchanged from the old Legend: severity chips appear on Habits only,
// harness chips on both tabs, a chip toggles a single EmphasisFilter, and clicking
// the lit chip clears it. Rendered under jsdom (house no-deps style).

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { FilterBar } from './FilterBar';
import type { OrbAgent, OrbSceneModel } from './orbTypes';

function model(harnesses: string[] = ['claude_code']): OrbSceneModel {
  const agents: OrbAgent[] = harnesses.map((h, i) => ({
    id: `${h}-${i}`,
    harness: h,
    label: '',
    glyph: '',
    color: '',
    sessions: 0,
    eventCount: 0,
    totalLoad: 0,
  }));
  return { agents, issues: [], links: [], guidance: { doItems: [], stopItems: [] } };
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

describe('FilterBar', () => {
  it('shows severity chips on Habits and hides them on Radar (harness chips on both)', () => {
    const habits = render(<FilterBar tab="habits" model={model()} filter={null} onFilter={() => {}} />);
    expect(habits.querySelectorAll('.wd-chip-sev').length).toBe(4);
    expect(habits.querySelectorAll('.wd-chip-harness').length).toBeGreaterThan(0);

    act(() => root!.render(<FilterBar tab="radar" model={model()} filter={null} onFilter={() => {}} />));
    expect(habits.querySelectorAll('.wd-chip-sev').length).toBe(0);
    expect(habits.querySelectorAll('.wd-chip-harness').length).toBeGreaterThan(0);
  });

  it('toggles a harness filter on click, and clears it when the lit chip is clicked again', () => {
    const onFilter = vi.fn();
    const el = render(<FilterBar tab="radar" model={model(['claude_code'])} filter={null} onFilter={onFilter} />);
    const chip = el.querySelector('.wd-chip-harness') as HTMLButtonElement;
    act(() => chip.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(onFilter).toHaveBeenCalledWith({ kind: 'harness', harness: 'claude_code' });

    onFilter.mockClear();
    act(() =>
      root!.render(
        <FilterBar tab="radar" model={model(['claude_code'])} filter={{ kind: 'harness', harness: 'claude_code' }} onFilter={onFilter} />,
      ),
    );
    const active = el.querySelector('.wd-chip-harness') as HTMLButtonElement;
    expect(active.getAttribute('aria-pressed')).toBe('true');
    act(() => active.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(onFilter).toHaveBeenCalledWith(null);
  });

  it('emits a severity filter from a severity chip on the Habits tab', () => {
    const onFilter = vi.fn();
    const el = render(<FilterBar tab="habits" model={model()} filter={null} onFilter={onFilter} />);
    const sev = el.querySelector('.wd-chip-sev') as HTMLButtonElement;
    act(() => sev.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(onFilter).toHaveBeenCalledTimes(1);
    expect(onFilter.mock.calls[0][0]).toMatchObject({ kind: 'severity' });
  });

  it('falls back to a single Unknown harness chip when no agents are present', () => {
    const el = render(<FilterBar tab="radar" model={model([])} filter={null} onFilter={() => {}} />);
    const chips = el.querySelectorAll('.wd-chip-harness');
    expect(chips.length).toBe(1);
    expect((chips[0].textContent ?? '').toLowerCase()).toContain('unknown');
  });
});
