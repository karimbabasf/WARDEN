// @vitest-environment jsdom
//
// The switcher exists because watching a peer used to be one-way: the camera trucked
// onto their board the instant a frame arrived and there was no control that brought
// it back. So the tests that matter are "it is there when there are two boards", "it
// is absent when there is only one", and "both directions are reachable".

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { ConstellationSwitcher } from './ConstellationSwitcher';

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

const props = {
  localCount: 4,
  peerLabel: "Askhat's machine",
  peerCount: 2,
  target: 'peer' as const,
  onTarget: () => {},
};

describe('ConstellationSwitcher', () => {
  it('names both boards and their real agent counts', () => {
    const el = render(<ConstellationSwitcher {...props} />);
    const segs = el.querySelectorAll('.wd-constellation-seg');
    expect(segs).toHaveLength(2);
    expect(segs[0].textContent).toContain('Your agents');
    expect(segs[0].textContent).toContain('4 agents');
    expect(segs[1].textContent).toContain("Askhat's machine");
    expect(segs[1].textContent).toContain('2 agents');
  });

  it('marks the framed board as the checked option', () => {
    const el = render(<ConstellationSwitcher {...props} target="local" />);
    const segs = el.querySelectorAll('.wd-constellation-seg');
    expect(segs[0].getAttribute('aria-checked')).toBe('true');
    expect(segs[1].getAttribute('aria-checked')).toBe('false');
    expect(segs[0].classList.contains('is-active')).toBe(true);
  });

  // The bug this whole control fixes: from the peer's board there must be a way home.
  it('switches in both directions', () => {
    const onTarget = vi.fn();
    const el = render(<ConstellationSwitcher {...props} target="peer" onTarget={onTarget} />);
    const segs = el.querySelectorAll<HTMLButtonElement>('.wd-constellation-seg');

    act(() => segs[0].click());
    expect(onTarget).toHaveBeenCalledWith('local');

    act(() => segs[1].click());
    expect(onTarget).toHaveBeenCalledWith('peer');
  });

  it('renders nothing when there is only one board to look at', () => {
    // Nobody watched at all.
    expect(render(<ConstellationSwitcher {...props} peerLabel={null} />).children).toHaveLength(0);
    act(() => root?.unmount());
    container?.remove();

    // Watched, but they have sent no frame worth drawing: a switcher pointing at an
    // empty patch of space would read as "they have no agents", which is a claim
    // WARDEN cannot make from silence.
    expect(render(<ConstellationSwitcher {...props} peerCount={0} />).children).toHaveLength(0);
  });

  it('counts one agent in the singular', () => {
    const el = render(<ConstellationSwitcher {...props} localCount={1} peerCount={1} />);
    const segs = el.querySelectorAll('.wd-constellation-seg');
    expect(segs[0].textContent).toContain('1 agent');
    expect(segs[0].textContent).not.toContain('1 agents');
  });
});
