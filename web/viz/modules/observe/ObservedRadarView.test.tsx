// @vitest-environment jsdom
//
// ObservedRadarView is the ONE renderer for a redacted `ObservedState` frame, shared by
// the observer dock and the host preview. The load-bearing assertion is that it renders
// a full, readable agent card from ONLY the shape/number fields `ObservedAgent` carries:
// no name, no label, no path, no prompt, no cost ever appears, because none of those
// fields exist on the type to begin with.

import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { ObservedRadarView } from './ObservedRadarView';
import type { ObservedAgent, ObservedState } from '@/viz/shared/types/observedTypes';

function agentFixture(over: Partial<ObservedAgent> = {}): ObservedAgent {
  return {
    id: 'a1b2c3d4e5f6',
    parentId: null,
    harness: 'claude_code',
    depth: 0,
    status: 'working',
    project: 'project A',
    role: 'Explore',
    model: 'claude-opus-4-8',
    contextTokens: 172_000,
    maxTokens: 200_000,
    fillPct: 0.86,
    contextRows: [{ key: 'messages', tokens: 100_000, percentX100: 5000 }],
    childCount: 1,
    ageSecs: 300,
    currentAction: { kind: 'read', tool: 'Read', elapsedSecs: 12 },
    recentActivity: [{ kind: 'tool', secsAgo: 5 }],
    team: { id: 'team-hash', memberCount: 3, isLead: true, memberType: 'Explore' },
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

describe('ObservedRadarView', () => {
  it('shows a waiting state when no frame has landed yet (state === null)', () => {
    const el = render(<ObservedRadarView state={null} title="Peer" />);
    expect(el.textContent).toContain('Waiting for the first frame');
    expect(el.querySelector('.wd-observe-agent-list')).toBeNull();
  });

  it('shows an honest empty state when the frame has zero agents', () => {
    const state: ObservedState = { generatedAt: 'now', agents: [], truncated: false };
    const el = render(<ObservedRadarView state={state} title="Peer" />);
    expect(el.textContent).toContain('No active agents');
  });

  it('shows the truncated banner only when the frame says truncated', () => {
    const state: ObservedState = { generatedAt: 'now', agents: [agentFixture()], truncated: true };
    const el = render(<ObservedRadarView state={state} title="Peer" />);
    expect(el.querySelector('.wd-observe-truncated')).not.toBeNull();
  });

  it('renders shape and number fields for each agent', () => {
    const state: ObservedState = { generatedAt: 'now', agents: [agentFixture()], truncated: false };
    const el = render(<ObservedRadarView state={state} title="Peer" />);
    expect(el.querySelectorAll('.wd-observe-agent')).toHaveLength(1);
    expect(el.textContent).toContain('project A');
    expect(el.textContent).toContain('Explore');
    expect(el.textContent).toContain('86%');
    expect(el.textContent).toContain('[a1b2c3d4]'); // truncated hash, bracketed as opaque data
    expect(el.textContent).toContain('Read');
    expect(el.textContent).toContain('Team');
  });

  it('never renders a name, label, path or cost, because the type carries none', () => {
    const state: ObservedState = { generatedAt: 'now', agents: [agentFixture()], truncated: false };
    const el = render(<ObservedRadarView state={state} title="Peer" />);
    const text = el.textContent ?? '';
    expect(text).not.toMatch(/\$\d/); // no cost figure
    expect(text).not.toMatch(/\/Users|~\//); // no filesystem path
    expect(text.toLowerCase()).not.toContain('karim');
  });

  it('indents deeper agents further, so the hierarchy is visible without naming it', () => {
    const state: ObservedState = {
      generatedAt: 'now',
      agents: [agentFixture({ id: 'root', depth: 0 }), agentFixture({ id: 'child', depth: 1, parentId: 'root' })],
      truncated: false,
    };
    const el = render(<ObservedRadarView state={state} title="Peer" />);
    const [rootLi, childLi] = Array.from(el.querySelectorAll('.wd-observe-agent')) as HTMLElement[];
    const rootPad = parseInt(rootLi.style.paddingLeft, 10);
    const childPad = parseInt(childLi.style.paddingLeft, 10);
    expect(childPad).toBeGreaterThan(rootPad);
  });
});
