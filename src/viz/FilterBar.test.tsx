// @vitest-environment jsdom
//
// FilterBar is the harness emphasis filter in its own bottom-centre dock. Severity
// buckets were a Habits-only signal and went with the Habits scene — RADAR filters
// by harness alone. A chip toggles a single EmphasisFilter, and clicking the lit
// chip clears it. Rendered under jsdom (house no-deps style).

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
  it('renders a harness chip per present harness and no severity chips', () => {
    const el = render(<FilterBar model={model(['claude_code', 'codex'])} filter={null} onFilter={() => {}} />);
    expect(el.querySelectorAll('.wd-chip-sev').length).toBe(0);
    expect(el.querySelectorAll('.wd-chip-harness').length).toBe(2);
  });

  it('toggles a harness filter on click, and clears it when the lit chip is clicked again', () => {
    const onFilter = vi.fn();
    const el = render(<FilterBar model={model(['claude_code'])} filter={null} onFilter={onFilter} />);
    const chip = el.querySelector('.wd-chip-harness') as HTMLButtonElement;
    act(() => chip.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(onFilter).toHaveBeenCalledWith({ kind: 'harness', harness: 'claude_code' });

    onFilter.mockClear();
    act(() =>
      root!.render(
        <FilterBar model={model(['claude_code'])} filter={{ kind: 'harness', harness: 'claude_code' }} onFilter={onFilter} />,
      ),
    );
    const active = el.querySelector('.wd-chip-harness') as HTMLButtonElement;
    expect(active.getAttribute('aria-pressed')).toBe('true');
    act(() => active.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(onFilter).toHaveBeenCalledWith(null);
  });

  it('falls back to a single Unknown harness chip when no agents are present', () => {
    const el = render(<FilterBar model={model([])} filter={null} onFilter={() => {}} />);
    const chips = el.querySelectorAll('.wd-chip-harness');
    expect(chips.length).toBe(1);
    expect((chips[0].textContent ?? '').toLowerCase()).toContain('unknown');
  });
});
