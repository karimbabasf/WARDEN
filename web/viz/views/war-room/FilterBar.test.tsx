// @vitest-environment jsdom
//
// FilterBar is the harness emphasis filter dock over the radar board. It renders one
// chip per distinct live harness (plus a quiet Unknown fallback when the board is
// empty), a chip toggles a single EmphasisFilter, and clicking the lit chip clears it.
// Rendered under jsdom (house no-deps style).

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { FilterBar } from './FilterBar';
import type { EmphasisFilter } from '@/viz/shared/lib/emphasis';

let container: HTMLDivElement | null = null;
let root: Root | null = null;

function render(node: ReactNode): HTMLElement {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(node));
  return container;
}

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  container = null;
  root = null;
});

describe('FilterBar', () => {
  it('renders one chip per distinct harness', () => {
    const el = render(
      <FilterBar
        agents={[{ harness: 'claude_code' }, { harness: 'codex' }, { harness: 'claude_code' }]}
        filter={null}
        onFilter={() => {}}
      />,
    );
    expect(el.querySelectorAll('.wd-chip-harness').length).toBe(2);
  });

  it('falls back to a single quiet Unknown chip when the board is empty', () => {
    const el = render(<FilterBar agents={[]} filter={null} onFilter={() => {}} />);
    expect(el.querySelectorAll('.wd-chip-harness').length).toBe(1);
  });

  it('toggles the harness filter on when a chip is clicked', () => {
    const onFilter = vi.fn();
    const el = render(<FilterBar agents={[{ harness: 'codex' }]} filter={null} onFilter={onFilter} />);
    const chip = el.querySelector('.wd-chip-harness') as HTMLButtonElement;
    act(() => chip.click());
    expect(onFilter).toHaveBeenCalledWith({ kind: 'harness', harness: 'codex' });
  });

  it('clears the filter when the already-lit chip is clicked', () => {
    const onFilter = vi.fn();
    const lit: EmphasisFilter = { kind: 'harness', harness: 'codex' };
    const el = render(<FilterBar agents={[{ harness: 'codex' }]} filter={lit} onFilter={onFilter} />);
    const chip = el.querySelector('.wd-chip-harness.is-active') as HTMLButtonElement;
    expect(chip).not.toBeNull();
    act(() => chip.click());
    expect(onFilter).toHaveBeenCalledWith(null);
  });
});
