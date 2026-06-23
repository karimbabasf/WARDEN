// @vitest-environment jsdom
//
// Render test for the diagnosis screen (Task 9). The global vitest env is `node`
// (the bridge/timing/recorder suites are pure logic), so this file opts into
// jsdom via the docblock above — only the diagnosis DOM renderer needs a
// document. We assert the SHAPE of the rendered payload (severity meter, cost
// ledger, harness badge, do/stop/narrative, evidence drill-down, fix preview),
// never the styling — pixels are eyeballed live in the overlay.

import { describe, it, expect, beforeEach } from 'vitest';
import { renderDiagnosis, renderFixPreview, type Diagnosis, type Finding, type FixPreview } from './diagnosis';

function finding(over: Partial<Finding> = {}): Finding {
  return {
    id: 'f-' + (over.pattern_id ?? 'x'),
    pattern_id: 'CONTEXT_BLOAT',
    title: 'Unbounded context growth',
    severity: 4,
    frequency: 0.62,
    confidence: 0.81,
    est_cost_tokens: 42000,
    est_cost_minutes: 9,
    rationale: 'Sessions never compact; the window fills with stale tool output.',
    evidence: [
      { session_id: 'sess-abcdef0123', turn_id: 'turn-77', event_id: 'evt-12', quote: '38 file reads, no compaction before the 180k ceiling' },
    ],
    status: 'confirmed',
    verifier_verdict: 'confirmed: matches 3 of 4 heuristics',
    ...over,
  };
}

function diagnosis(over: Partial<Diagnosis> = {}): Diagnosis {
  return {
    id: 'diag-998877',
    created_at: new Date().toISOString(),
    ranked_findings: [
      finding({ pattern_id: 'CONTEXT_BLOAT', title: 'Unbounded context growth', severity: 4 }),
      finding({
        pattern_id: 'NO_DELEGATION',
        title: 'No subagent delegation',
        severity: 2,
        evidence: [{ session_id: 'sess-codex9911', turn_id: 'turn-3', event_id: 'evt-5', quote: '0 Task spawns across 41 tool calls' }],
      }),
    ],
    do_items: ['Compact at 60% context', 'Delegate file sweeps to a subagent'],
    stop_items: ['Re-reading the same file each turn'],
    narrative: 'Two structural holes are quietly taxing every long session.',
    detector_only: false,
    ...over,
  };
}

// The renderer is dependency-injected with the harness id per finding (the web
// `Finding` has no harness field; the screen tags each finding by harness when
// the caller knows it). For the test we map the two findings to claude/codex.
function harnessOf(f: Finding): string {
  return f.pattern_id === 'NO_DELEGATION' ? 'codex' : 'claude_code';
}

let host: HTMLElement;
beforeEach(() => {
  document.body.innerHTML = '';
  host = document.createElement('div');
  document.body.appendChild(host);
});

describe('renderDiagnosis', () => {
  it('renders one ranked hole per finding with a 1-5 severity meter, frequency and cost ledger', () => {
    renderDiagnosis(host, diagnosis(), { harnessOf });

    const holes = host.querySelectorAll('.diag-hole');
    expect(holes).toHaveLength(2);

    // Severity meter: 5 discrete ticks, the right number filled for each finding.
    const firstMeter = holes[0].querySelector('.sev-meter')!;
    expect(firstMeter).toBeTruthy();
    expect(firstMeter.querySelectorAll('.sev-tick')).toHaveLength(5);
    expect(firstMeter.querySelectorAll('.sev-tick.on')).toHaveLength(4); // severity 4
    expect(firstMeter.getAttribute('aria-label')).toMatch(/severity 4 of 5/i);

    const secondMeter = holes[1].querySelector('.sev-meter')!;
    expect(secondMeter.querySelectorAll('.sev-tick.on')).toHaveLength(2); // severity 2

    // Frequency + est cost (tokens AND minutes) surfaced in the ledger.
    const ledger = holes[0].textContent ?? '';
    expect(ledger).toMatch(/62%/); // frequency 0.62
    expect(ledger).toMatch(/42,?000/); // est tokens
    expect(ledger).toMatch(/9\s*min/i); // est minutes

    // One-line summary present.
    expect(host.textContent).toContain('Sessions never compact');
  });

  it('tags every finding with a harness badge: colour PAIRED with glyph + label', () => {
    renderDiagnosis(host, diagnosis(), { harnessOf });
    const holes = host.querySelectorAll('.diag-hole');

    const b0 = holes[0].querySelector('.harness-badge')!;
    expect(b0.textContent).toContain('◆');
    expect(b0.textContent).toContain('Claude');

    const b1 = holes[1].querySelector('.harness-badge')!;
    expect(b1.textContent).toContain('▲');
    expect(b1.textContent).toContain('Codex');
  });

  it('renders the Do list, Stop list and narrative', () => {
    renderDiagnosis(host, diagnosis(), { harnessOf });
    const doList = host.querySelector('.diag-do')!;
    const stopList = host.querySelector('.diag-stop')!;
    expect(doList.textContent).toContain('Compact at 60% context');
    expect(stopList.textContent).toContain('Re-reading the same file');
    expect(host.querySelector('.diag-narrative')!.textContent).toContain('quietly taxing');
  });

  it('expands a finding to show its evidence refs (session · turn · quote)', () => {
    renderDiagnosis(host, diagnosis(), { harnessOf });
    const firstHole = host.querySelector('.diag-hole')!;

    // Collapsed by default: evidence list is not yet populated/visible.
    const toggle = firstHole.querySelector<HTMLButtonElement>('.evidence-toggle')!;
    expect(toggle).toBeTruthy();
    expect(firstHole.querySelector('.evidence-item')).toBeNull();

    toggle.click();

    const items = firstHole.querySelectorAll('.evidence-item');
    expect(items).toHaveLength(1);
    const txt = items[0].textContent ?? '';
    expect(txt).toContain('sess-abcde'); // session id (shortened)
    expect(txt).toContain('turn-77'); // turn id
    expect(txt).toContain('38 file reads'); // the stored quote
  });

  it('labels findings detector-only when the diagnosis was produced without the API', () => {
    renderDiagnosis(host, diagnosis({ detector_only: true }), { harnessOf });
    expect(host.textContent?.toLowerCase()).toContain('detector-only');
    expect(host.textContent?.toLowerCase()).toMatch(/no api key|budget/);
  });
});

describe('renderFixPreview', () => {
  const preview: FixPreview = {
    finding_id: 'f-CONTEXT_BLOAT',
    pattern_id: 'CONTEXT_BLOAT',
    target_path: '/Users/me/.claude/CLAUDE.md',
    diff: '--- a/CLAUDE.md\n+++ b/CLAUDE.md\n@@ -1,3 +1,4 @@\n context guidance\n-keep everything in one window\n+Compact at 60% context\n+Delegate file sweeps to a subagent\n',
    applied: false,
  };

  it('renders the unified diff with +/- lines styled and the target path', () => {
    const block = renderFixPreview(host, preview);
    expect(host.contains(block)).toBe(true);
    expect(block.textContent).toContain('CLAUDE.md');
    expect(block.querySelectorAll('.diff-add').length).toBeGreaterThanOrEqual(1);
    expect(block.querySelectorAll('.diff-del').length).toBeGreaterThanOrEqual(1);
  });

  it('renders a DISABLED [Y/n] apply prompt labeled "apply coming in M4"', () => {
    const block = renderFixPreview(host, preview);
    const apply = block.querySelector<HTMLButtonElement>('.apply-prompt')!;
    expect(apply).toBeTruthy();
    expect(apply.disabled).toBe(true);
    expect(apply.textContent).toMatch(/\[Y\/n\]/);
    expect(block.textContent?.toLowerCase()).toContain('apply coming in m4');
  });
});
