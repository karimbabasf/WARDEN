// @vitest-environment jsdom
//
// ProfileScreen component tests — the three "evidence is inert" / "double
// source" / "dead-end hash" gaps closed 2026-07-02. Rendering follows the
// house no-deps convention (react-dom/client + act, no @testing-library —
// see RadarHoverCard.test.tsx): we mount real DOM and assert on textContent
// / attributes / click behaviour.
//
// ProfileScreen itself calls `invoke` on mount (Tauri IPC), which has no
// backend in a jsdom test run, so these tests drive the exported
// PRESENTATIONAL subcomponents directly with fixture props — exactly the
// "smallest change that makes them testable" escape hatch the spec allows.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { ClaimRow, EfficiencyPanel, HashBadge, LeaksPanel } from './ProfileScreen';
import type { Claim, EfficiencyScore, EvidenceRef, Leak } from './types';

// ── fixtures ────────────────────────────────────────────────────────────────

function evidenceFixture(over: Partial<EvidenceRef> = {}): EvidenceRef {
  return {
    session_id: 'sess-abc123',
    turn_id: 'turn-7',
    event_id: 'evt-1',
    quote: 'ran the full test suite before committing',
    source_path: '/Users/karimbaba/WARDEN/src-tauri/src/store.rs',
    ...over,
  };
}

function claimFixture(over: Partial<Claim> = {}): Claim {
  return {
    text: 'Verifies before claiming completion',
    confidence: 0.82,
    status: 'asserted',
    evidence: [
      evidenceFixture({ quote: 'ran the full test suite before committing', turn_id: 'turn-7' }),
      evidenceFixture({ quote: 'confirmed build output was green', turn_id: 'turn-9', session_id: 'sess-def456' }),
      evidenceFixture({ quote: 're-ran after a flaky failure to double check', turn_id: 'turn-11', session_id: 'sess-ghi789' }),
    ],
    ...over,
  };
}

function leakFixture(over: Partial<Leak> = {}): Leak {
  return {
    rank: 1,
    title: 'Repeats the same failing command without changing approach',
    est_cost_tokens: 4200,
    est_cost_minutes: 6,
    evidence: [
      evidenceFixture({ quote: 'retried cargo build a third time unchanged', turn_id: 'turn-2' }),
      evidenceFixture({ quote: 'same error, same command, no diagnosis', turn_id: 'turn-3', session_id: 'sess-def456' }),
      evidenceFixture({ quote: 'fourth attempt still identical', turn_id: 'turn-4', session_id: 'sess-ghi789' }),
    ],
    ...over,
  };
}

function efficiencyFixture(over: Partial<EfficiencyScore> = {}): EfficiencyScore {
  return {
    headline: 0.71,
    rubric_version: 'v3',
    families: [
      { key: 'verification', sub_score: 0.8, weight: 0.3 },
      { key: 'delegation', sub_score: 0.6, weight: 0.2 },
    ],
    session_count: 42,
    ...over,
  };
}

// ── render harness (matches RadarHoverCard.test.tsx exactly) ──────────────

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

// ── GAP 1: evidence toggle (claims) ────────────────────────────────────────

describe('ClaimRow — evidence toggle', () => {
  it('hides all evidence quotes before the toggle is clicked', () => {
    const el = render(
      <ul>
        <ClaimRow claim={claimFixture()} />
      </ul>,
    );
    const text = el.textContent ?? '';
    expect(text).not.toContain('confirmed build output was green');
    expect(text).not.toContain('re-ran after a flaky failure to double check');
  });

  it('reveals every evidence ref (not just the first) once the toggle is clicked', () => {
    const el = render(
      <ul>
        <ClaimRow claim={claimFixture()} />
      </ul>,
    );
    const toggle = el.querySelector('button[aria-expanded]') as HTMLButtonElement;
    expect(toggle).toBeTruthy();
    act(() => {
      toggle.click();
    });
    const text = el.textContent ?? '';
    expect(text).toContain('ran the full test suite before committing');
    expect(text).toContain('confirmed build output was green');
    expect(text).toContain('re-ran after a flaky failure to double check');
  });

  it('shows a source tail (last 2 path segments) and turn id for each revealed evidence ref', () => {
    const el = render(
      <ul>
        <ClaimRow claim={claimFixture()} />
      </ul>,
    );
    const toggle = el.querySelector('button[aria-expanded]') as HTMLButtonElement;
    act(() => {
      toggle.click();
    });
    const text = el.textContent ?? '';
    // last 2 segments of /Users/karimbaba/WARDEN/src-tauri/src/store.rs
    expect(text).toContain('src/store.rs');
    expect(text).toContain('turn-7');
    expect(text).toContain('turn-9');
    expect(text).toContain('turn-11');
  });

  it('toggle button is keyboard accessible (a real <button>, aria-expanded flips)', () => {
    const el = render(
      <ul>
        <ClaimRow claim={claimFixture()} />
      </ul>,
    );
    const toggle = el.querySelector('button[aria-expanded]') as HTMLButtonElement;
    expect(toggle.tagName).toBe('BUTTON');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    act(() => {
      toggle.click();
    });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
  });

  it('truncates a long quote to ~140 chars when expanded', () => {
    const longQuote = 'x'.repeat(300);
    const el = render(
      <ul>
        <ClaimRow claim={claimFixture({ evidence: [evidenceFixture({ quote: longQuote })] })} />
      </ul>,
    );
    const toggle = el.querySelector('button[aria-expanded]') as HTMLButtonElement;
    act(() => {
      toggle.click();
    });
    const text = el.textContent ?? '';
    // full 300-char run should NOT appear verbatim; a truncated (~140) run should.
    expect(text).not.toContain(longQuote);
    expect(text).toContain('x'.repeat(100)); // well under the truncation bound
  });

  it('handles null/optional evidence fields without crashing (quote null, source_path null)', () => {
    const el = render(
      <ul>
        <ClaimRow
          claim={claimFixture({
            evidence: [evidenceFixture({ quote: null, source_path: null, turn_id: null })],
          })}
        />
      </ul>,
    );
    const toggle = el.querySelector('button[aria-expanded]') as HTMLButtonElement;
    expect(() =>
      act(() => {
        toggle.click();
      }),
    ).not.toThrow();
  });

  it('renders no toggle when a claim has zero evidence', () => {
    const el = render(
      <ul>
        <ClaimRow claim={claimFixture({ evidence: [] })} />
      </ul>,
    );
    expect(el.querySelector('button[aria-expanded]')).toBeNull();
  });
});

// ── GAP 1: evidence toggle (leaks) — same reusable component ──────────────

describe('LeaksPanel — evidence toggle', () => {
  it('hides all evidence quotes before the toggle is clicked', () => {
    const el = render(<LeaksPanel leaks={[leakFixture()]} />);
    const text = el.textContent ?? '';
    expect(text).not.toContain('same error, same command, no diagnosis');
    expect(text).not.toContain('fourth attempt still identical');
  });

  it('reveals every evidence ref for the leak once the toggle is clicked', () => {
    const el = render(<LeaksPanel leaks={[leakFixture()]} />);
    const toggle = el.querySelector('button[aria-expanded]') as HTMLButtonElement;
    expect(toggle).toBeTruthy();
    act(() => {
      toggle.click();
    });
    const text = el.textContent ?? '';
    expect(text).toContain('retried cargo build a third time unchanged');
    expect(text).toContain('same error, same command, no diagnosis');
    expect(text).toContain('fourth attempt still identical');
  });

  it('multiple leaks each get their own independent toggle', () => {
    const el = render(
      <LeaksPanel
        leaks={[
          leakFixture({ rank: 1, title: 'Leak A', evidence: [evidenceFixture({ quote: 'quote-A-only' })] }),
          leakFixture({ rank: 2, title: 'Leak B', evidence: [evidenceFixture({ quote: 'quote-B-only' })] }),
        ]}
      />,
    );
    const toggles = el.querySelectorAll('button[aria-expanded]');
    expect(toggles.length).toBe(2);
    act(() => {
      (toggles[0] as HTMLButtonElement).click();
    });
    const text = el.textContent ?? '';
    expect(text).toContain('quote-A-only');
    expect(text).not.toContain('quote-B-only'); // second toggle untouched
  });
});

// ── GAP 2: efficiency double-source ────────────────────────────────────────

describe('EfficiencyPanel — eff vs profile.efficiency fallback', () => {
  it('renders from the standalone eff when present', () => {
    const el = render(
      <EfficiencyPanel eff={efficiencyFixture({ headline: 0.71, session_count: 42 })} profileEff={null} />,
    );
    const text = el.textContent ?? '';
    expect(text).toContain('71');
    expect(text).toContain('42 sessions');
  });

  it('falls back to profile.efficiency when the standalone score is null', () => {
    const el = render(
      <EfficiencyPanel eff={null} profileEff={efficiencyFixture({ headline: 0.55, session_count: 9 })} />,
    );
    const text = el.textContent ?? '';
    expect(text).toContain('55');
    expect(text).toContain('9 sessions');
  });

  it('renders nothing (or an empty shell) when both are null', () => {
    const el = render(<EfficiencyPanel eff={null} profileEff={null} />);
    // Must not throw and must not fabricate a headline number.
    expect(el.textContent ?? '').not.toMatch(/\d+\s*\/\s*100/);
  });

  it('prefers the fresher standalone eff over profileEff when both exist and agree', () => {
    const el = render(
      <EfficiencyPanel
        eff={efficiencyFixture({ headline: 0.9, rubric_version: 'v3', families: [] })}
        profileEff={efficiencyFixture({ headline: 0.2, rubric_version: 'v3', families: [] })}
      />,
    );
    const text = el.textContent ?? '';
    expect(text).toContain('90 / 100'); // the standalone (fresher) headline wins
    expect(text).not.toContain('20 / 100');
    expect(text).toContain('0.90'); // raw 0..1 form of the same resolved value
    expect(text).not.toContain('0.20');
  });

  it('shows a rubric-drift warning when eff and profileEff rubric_version differ', () => {
    const el = render(
      <EfficiencyPanel
        eff={efficiencyFixture({ rubric_version: 'v4' })}
        profileEff={efficiencyFixture({ rubric_version: 'v3' })}
      />,
    );
    const text = el.textContent ?? '';
    expect(text.toLowerCase()).toContain('rubric drift');
    expect(text).toContain('v4');
    expect(text).toContain('v3');
  });

  it('shows no rubric-drift warning when versions are equal', () => {
    const el = render(
      <EfficiencyPanel
        eff={efficiencyFixture({ rubric_version: 'v3' })}
        profileEff={efficiencyFixture({ rubric_version: 'v3' })}
      />,
    );
    const text = el.textContent ?? '';
    expect(text.toLowerCase()).not.toContain('rubric drift');
  });

  it('shows no rubric-drift warning when only one source exists (nothing to compare)', () => {
    const el = render(<EfficiencyPanel eff={efficiencyFixture({ rubric_version: 'v4' })} profileEff={null} />);
    const text = el.textContent ?? '';
    expect(text.toLowerCase()).not.toContain('rubric drift');
  });
});

// ── GAP 3: data_hash dead-end ───────────────────────────────────────────────

describe('HashBadge — full hash + click-to-copy', () => {
  const fullHash = 'a1b2c3d4e5f60718293a4b5c6d7e8f9012345678';

  it('carries the FULL hash in a title attribute (not just the truncated display text)', () => {
    const el = render(<HashBadge hash={fullHash} />);
    const el2 = el.querySelector('[title]') as HTMLElement;
    expect(el2).toBeTruthy();
    expect(el2.getAttribute('title')).toBe(fullHash);
  });

  it('displays only a truncated (8-char) hash as visible text', () => {
    const el = render(<HashBadge hash={fullHash} />);
    expect(el.textContent ?? '').toContain(fullHash.slice(0, 8));
    expect(el.textContent ?? '').not.toContain(fullHash);
  });

  it('copies the full hash to the clipboard on click and flips to a brief "copied" state', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.assign(navigator, { clipboard: { writeText } });

    const el = render(<HashBadge hash={fullHash} />);
    const clickable = el.querySelector('[title]') as HTMLElement;

    await act(async () => {
      clickable.click();
      await Promise.resolve();
    });

    expect(writeText).toHaveBeenCalledWith(fullHash);
    expect((el.textContent ?? '').toLowerCase()).toContain('copied');

    // @ts-expect-error test cleanup of a test-only global augmentation
    delete navigator.clipboard;
  });

  it('does not throw when navigator.clipboard is absent (guarded)', async () => {
    // jsdom does not implement navigator.clipboard by default; simulate that
    // absence explicitly regardless of the environment's baseline.
    const original = (navigator as unknown as { clipboard?: unknown }).clipboard;
    // @ts-expect-error deliberately removing for the guard test
    delete navigator.clipboard;

    const el = render(<HashBadge hash={fullHash} />);
    const clickable = el.querySelector('[title]') as HTMLElement;

    await expect(
      act(async () => {
        clickable.click();
        await Promise.resolve();
      }),
    ).resolves.not.toThrow();

    if (original !== undefined) {
      Object.assign(navigator, { clipboard: original });
    }
  });
});

describe('module import sanity', () => {
  beforeEach(() => {
    // no-op — keeps beforeEach import used and suite symmetric with afterEach
  });

  it('exports the presentational subcomponents needed for direct testing', () => {
    expect(typeof ClaimRow).toBe('function');
    expect(typeof LeaksPanel).toBe('function');
    expect(typeof EfficiencyPanel).toBe('function');
    expect(typeof HashBadge).toBe('function');
  });
});
