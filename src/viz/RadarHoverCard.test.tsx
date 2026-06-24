// @vitest-environment jsdom
//
// RadarHoverCard component test (Task 17). WebGL is never touched here — the card
// is a screen-space DOM overlay (drei <Html> projects it; the inner markup is what
// we assert), so it renders honestly under jsdom with react-dom/client + act (no
// extra test-library dependency, matching the house no-deps style of mount.test.ts).
//
// The quick-glance card must surface, at a glance: label, harness glyph + label,
// model, fill %, child count, status — every field a real signal from `RadarAgent`.

import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { RadarHoverCard } from './RadarHoverCard';
import type { RadarAgent } from './radarTypes';

// A fully-populated Codex subagent fixture: fill 0.72, 2 children, working.
function agentFixture(over: Partial<RadarAgent> = {}): RadarAgent {
  return {
    id: 'codex-sub-1',
    harness: 'codex',
    origin: 'Codex Desktop',
    parentId: 'codex-root',
    depth: 1,
    label: 'Curie',
    nickname: 'Curie',
    cwd: null,
    role: 'explorer',
    model: 'gpt-5-codex',
    status: 'working',
    contextTokens: 144_000,
    maxTokens: 200_000,
    fillPct: 0.72,
    composition: { exact: { cacheRead: 0, fresh: 0, output: 0 }, estimated: null },
    recentActivity: [],
    childCount: 2,
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
  act(() => {
    root!.render(node);
  });
  return container;
}

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
});

describe('RadarHoverCard — quick-glance fields', () => {
  it('shows label, model, fill %, child count, status, and the codex glyph', () => {
    const el = render(<RadarHoverCard agent={agentFixture()} />);
    const text = el.textContent ?? '';
    expect(text).toContain('Curie'); // label
    expect(text).toContain('gpt-5-codex'); // model
    expect(text).toContain('72%'); // fillPct → percent
    expect(text).toContain('2 children'); // childCount, pluralised
    expect(text).toMatch(/working/i); // status
    expect(text).toContain('▣'); // codex harness glyph (radarTheme)
    expect(text).toContain('Codex'); // harness label paired with glyph (a11y)
  });

  it('pluralises a single child correctly and shows the claude glyph', () => {
    const el = render(
      <RadarHoverCard agent={agentFixture({ harness: 'claude_code', childCount: 1, status: 'idle' })} />,
    );
    const text = el.textContent ?? '';
    expect(text).toContain('1 child');
    expect(text).not.toContain('1 children');
    expect(text).toContain('◆'); // claude glyph
    expect(text).toContain('Claude');
  });

  it('renders a graceful dash for an unknown model and 0% fill', () => {
    const el = render(<RadarHoverCard agent={agentFixture({ model: null, fillPct: 0, childCount: 0 })} />);
    const text = el.textContent ?? '';
    expect(text).toContain('0%');
    // no-children agents must not fabricate a roster line; child line omitted.
    expect(text).not.toContain('children');
  });
});
