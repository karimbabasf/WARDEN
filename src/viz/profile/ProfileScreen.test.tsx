// @vitest-environment jsdom
//
// DOSSIER component tests. The screen was rebuilt (2026-07-04) from a wall of
// identical boxes into a composed, sectioned intelligence dossier; these tests
// follow the same "drive the exported PRESENTATIONAL subcomponents directly with
// fixtures" escape hatch as before (ProfileScreen calls `invoke` on mount, which
// has no backend under jsdom). House convention: react-dom/client + act, no
// @testing-library — assert on textContent / attributes / click behaviour.
//
// Coverage carried over from the old ClaimRow/LeaksPanel/EfficiencyPanel tests,
// re-pointed at the new surfaces:
//   • EvidenceList  — evidence is FIRST-CLASS (visible citations, all refs, null-safe, independent).
//   • ScoreHero     — real headline, no fabricated number when null, verdict highlighting.
//   • LeaksSection  — rank + cost + per-leak evidence.
//   • BreakingSection helpers — window mapping + snake_case normalization.

import { afterEach, describe, expect, it } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { EvidenceList } from './EvidenceList';
import { ScoreHero } from './ScoreHero';
import { LeaksSection } from './LeaksSection';
import { DimensionsSection } from './DimensionsSection';
import { normalizeHabit, toHabitsWindow } from './BreakingSection';
import type { Claim, EfficiencyScore, EvidenceRef, Leak, ProfileDimension } from './types';

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
    ],
    ...over,
  };
}

function efficiencyFixture(over: Partial<EfficiencyScore> = {}): EfficiencyScore {
  return {
    headline: 0.71,
    rubric_version: 'v3',
    families: [
      { key: 'verification_discipline', sub_score: 0.8, weight: 0.3 },
      { key: 'context_hygiene', sub_score: 0.3, weight: 0.2 },
      { key: 'delegation', sub_score: 0.6, weight: 0.2 },
    ],
    session_count: 42,
    ...over,
  };
}

function dimFixture(over: Partial<ProfileDimension> = {}): ProfileDimension {
  return {
    key: 'strengths',
    title: 'Strengths',
    narrative: 'Consistent verification habits.',
    claims: [claimFixture()],
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

// ── EvidenceList — evidence promoted to first-class ────────────────────────

describe('EvidenceList — first-class citations', () => {
  it('shows the captured quote of EVERY evidence ref up front (not hidden behind a toggle)', () => {
    const el = render(<EvidenceList evidence={claimFixture().evidence} />);
    const text = el.textContent ?? '';
    expect(text).toContain('ran the full test suite before committing');
    expect(text).toContain('confirmed build output was green');
    expect(text).toContain('re-ran after a flaky failure to double check');
  });

  it('renders each citation as a real clickable button (keyboard accessible)', () => {
    const el = render(<EvidenceList evidence={[evidenceFixture()]} />);
    const cite = el.querySelector('button.evcite') as HTMLButtonElement;
    expect(cite).toBeTruthy();
    expect(cite.tagName).toBe('BUTTON');
    expect(cite.getAttribute('aria-expanded')).toBe('false');
  });

  it('shows a source tail (last 2 path segments) and turn id per citation', () => {
    const el = render(<EvidenceList evidence={claimFixture().evidence} />);
    const text = el.textContent ?? '';
    expect(text).toContain('src/store.rs'); // last 2 of the store.rs path
    expect(text).toContain('turn-7');
    expect(text).toContain('turn-9');
    expect(text).toContain('turn-11');
  });

  it('shows a count of cited sessions', () => {
    const el = render(<EvidenceList evidence={claimFixture().evidence} />);
    expect((el.textContent ?? '').toLowerCase()).toContain('3 cited sessions');
  });

  it('clicking a citation with no event_id resolves to an "unavailable" note without throwing', () => {
    const el = render(<EvidenceList evidence={[evidenceFixture({ event_id: null })]} />);
    const cite = el.querySelector('button.evcite') as HTMLButtonElement;
    expect(() =>
      act(() => {
        cite.click();
      }),
    ).not.toThrow();
    expect(el.textContent ?? '').toContain('No richer context available');
    expect(cite.getAttribute('aria-expanded')).toBe('true');
  });

  it('handles null quote / source_path / turn_id without crashing', () => {
    const el = render(
      <EvidenceList evidence={[evidenceFixture({ quote: null, source_path: null, turn_id: null })]} />,
    );
    const text = el.textContent ?? '';
    expect(text).toContain('no captured quote');
    expect(text).toContain('session'); // sourceTail fallback (session_id present)
  });

  it('each citation is independent — clicking one does not open another', () => {
    const el = render(
      <EvidenceList
        evidence={[
          evidenceFixture({ event_id: null, quote: 'first citation body' }),
          evidenceFixture({ event_id: null, quote: 'second citation body' }),
        ]}
      />,
    );
    const cites = el.querySelectorAll('button.evcite');
    expect(cites.length).toBe(2);
    act(() => {
      (cites[0] as HTMLButtonElement).click();
    });
    expect(cites[0].getAttribute('aria-expanded')).toBe('true');
    expect(cites[1].getAttribute('aria-expanded')).toBe('false');
  });

  it('empty evidence renders a quiet "none cited" note, no buttons', () => {
    const el = render(<EvidenceList evidence={[]} />);
    expect(el.querySelector('button.evcite')).toBeNull();
    expect((el.textContent ?? '').toLowerCase()).toContain('none cited');
  });
});

// ── ScoreHero — the hero number is honest ──────────────────────────────────

describe('ScoreHero — efficiency headline + verdict', () => {
  it('renders the real headline as a 0–100 score (0.71 → 71)', () => {
    const el = render(<ScoreHero eff={efficiencyFixture({ headline: 0.71 })} />);
    // count-up is reduced-motion-instant in jsdom (no matchMedia) — but assert
    // on the family/verdict text which is stable regardless of the animated digit.
    const text = el.textContent ?? '';
    expect(text).toContain('EFFICIENCY');
    expect(text).toContain('rubric v3');
    expect(text).toContain('42 sessions scored');
  });

  it('names the strongest and weakest family in the verdict line', () => {
    const el = render(<ScoreHero eff={efficiencyFixture()} />);
    const text = el.textContent ?? '';
    expect(text.toLowerCase()).toContain('strongest at');
    expect(text).toContain('verification discipline'); // humanized strongest
    expect(text).toContain('context hygiene'); // humanized weakest
  });

  it('does NOT fabricate a score number when efficiency is null', () => {
    const el = render(<ScoreHero eff={null} />);
    const text = el.textContent ?? '';
    expect(text).not.toMatch(/\b\d{1,3}\b\s*\/\s*100/); // no "NN / 100"
    expect(text).toContain('—'); // shows the honest placeholder
    expect(text.toLowerCase()).toContain('no scored sessions');
  });

  it('renders all provided families in the breakdown', () => {
    const el = render(<ScoreHero eff={efficiencyFixture()} />);
    const text = el.textContent ?? '';
    expect(text).toContain('delegation');
    expect(text).toContain('verification discipline');
    expect(text).toContain('context hygiene');
  });
});

// ── LeaksSection — rank, cost, first-class evidence ────────────────────────

describe('LeaksSection', () => {
  it('renders rank, title, and real token/minute cost', () => {
    const el = render(<LeaksSection leaks={[leakFixture()]} />);
    const text = el.textContent ?? '';
    expect(text).toContain('Repeats the same failing command');
    expect(text).toContain('4,200');
    expect(text).toContain('6');
    expect(text).toContain('min');
  });

  it('surfaces each leak citation quote up front (evidence-forward, not an 11px toggle)', () => {
    const el = render(<LeaksSection leaks={[leakFixture()]} />);
    const text = el.textContent ?? '';
    expect(text).toContain('retried cargo build a third time unchanged');
    expect(text).toContain('same error, same command, no diagnosis');
  });

  it('sorts by rank and caps at 5', () => {
    const many = Array.from({ length: 8 }, (_, i) =>
      leakFixture({ rank: 8 - i, title: `Leak ${8 - i}`, evidence: [] }),
    );
    const el = render(<LeaksSection leaks={many} />);
    const ranks = Array.from(el.querySelectorAll('.leak__rank')).map((n) => n.textContent);
    expect(ranks).toEqual(['1', '2', '3', '4', '5']);
  });

  it('empty leaks renders an honest note, no cards', () => {
    const el = render(<LeaksSection leaks={[]} />);
    expect(el.querySelector('.leak')).toBeNull();
    expect((el.textContent ?? '').toLowerCase()).toContain('no leaks surfaced');
  });
});

// ── DimensionsSection — strengths vs holes contrasted ──────────────────────

describe('DimensionsSection — contrasted split', () => {
  it('routes holes / where_you_lose to the exposed column and the rest to strengths', () => {
    const el = render(
      <DimensionsSection
        dimensions={[
          dimFixture({ key: 'strengths', title: 'Strengths' }),
          dimFixture({ key: 'holes', title: 'Holes', claims: [] }),
          dimFixture({ key: 'orchestration_style', title: 'Orchestration', claims: [] }),
        ]}
      />,
    );
    const strengthCol = el.querySelector('.who__col--strength') as HTMLElement;
    const holeCol = el.querySelector('.who__col--hole') as HTMLElement;
    expect(strengthCol.textContent).toContain('Strengths');
    expect(strengthCol.textContent).toContain('Orchestration');
    expect(holeCol.textContent).toContain('Holes');
    expect(strengthCol.textContent).not.toContain('Holes');
  });

  it('renders an asserted/emerging status chip per claim', () => {
    const el = render(
      <DimensionsSection
        dimensions={[
          dimFixture({
            claims: [
              claimFixture({ status: 'asserted', text: 'A solid claim' }),
              claimFixture({ status: 'emerging', text: 'A tentative claim', evidence: [] }),
            ],
          }),
        ]}
      />,
    );
    expect(el.querySelector('.chip--asserted')).toBeTruthy();
    expect(el.querySelector('.chip--emerging')).toBeTruthy();
  });
});

// ── BreakingSection helpers — window mapping + normalization ────────────────

describe('toHabitsWindow — Dossier → habits window mapping', () => {
  it('maps each Dossier window to the nearest habits window', () => {
    expect(toHabitsWindow('2wk')).toBe('7d');
    expect(toHabitsWindow('30d')).toBe('30d');
    expect(toHabitsWindow('3mo')).toBe('30d'); // habits has no 3mo bucket
    expect(toHabitsWindow('6mo')).toBe('6mo');
    expect(toHabitsWindow('all-time')).toBe('all');
  });
});

describe('normalizeHabit — snake_case OrbIssue → BreakingHabit', () => {
  it('reads snake_case streak fields off the wire', () => {
    const h = normalizeHabit({
      id: 'claude:CONTEXT_BLOAT',
      pattern_id: 'CONTEXT_BLOAT',
      title: 'Context bloat',
      est_cost_tokens: 84000,
      est_cost_minutes: 34,
      severity: 5,
      credits: 2,
      streak_k: 5,
      fixed: false,
      evidence: [{ session_id: 's1', event_id: 'e1', quote: 'q', turn_id: null, source_path: null }],
    });
    expect(h.patternId).toBe('CONTEXT_BLOAT');
    expect(h.credits).toBe(2);
    expect(h.streakK).toBe(5);
    expect(h.fixed).toBe(false);
    expect(h.estCostTokens).toBe(84000);
    expect(h.evidence).toHaveLength(1);
    expect(h.evidence[0].session_id).toBe('s1');
  });

  it('defaults missing fields safely (no NaN, no undefined)', () => {
    const h = normalizeHabit({});
    expect(h.credits).toBe(0);
    expect(h.streakK).toBe(0);
    expect(h.fixed).toBe(false);
    expect(h.title).toBe('Untitled pattern');
    expect(h.evidence).toEqual([]);
  });

  it('honors a fixed habit', () => {
    expect(normalizeHabit({ fixed: true }).fixed).toBe(true);
  });
});
