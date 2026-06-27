// @vitest-environment jsdom
//
// chrome.tsx — M4 Forge approval-flow tests. Two layers:
//   1. Pure helpers (classifyDiffLine / provenanceLabel / shortStamp) — no DOM.
//   2. Component states of the DetailPanel apply/diff/revert flow + the ledger,
//      rendered through the public `Chrome` entry under jsdom (house no-deps
//      style: react-dom/client + act, mirroring RadarDetailPanel.test.tsx).
//
// The non-negotiable honesty anchor: every applied/reverted affordance is driven
// by the REAL Artifact.status threaded in — never fabricated. These tests assert
// the card flips on `applied`, returns on `reverted`, shows ALREADY APPLIED on an
// empty diff, and that the ledger lists only historic (applied/reverted) rows.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { Chrome, classifyDiffLine, provenanceLabel, shortStamp, type Artifact, type FixPreview } from './chrome';
import type { LayoutNode, OrbIssue, OrbSceneModel } from '@/viz/shared/types/orbTypes';
import type { SceneState } from '@/viz/shared/state/bridge';

// ── pure helpers ──────────────────────────────────────────────────────────────
describe('classifyDiffLine', () => {
  it('types add / del / hunk / file / context lines', () => {
    expect(classifyDiffLine('+ new guardrail line')).toBe('add');
    expect(classifyDiffLine('- old line')).toBe('del');
    expect(classifyDiffLine('@@ -1,3 +1,5 @@')).toBe('hunk');
    expect(classifyDiffLine('+++ b/CLAUDE.md')).toBe('file');
    expect(classifyDiffLine('--- a/CLAUDE.md')).toBe('file');
    expect(classifyDiffLine(' unchanged context')).toBe('ctx');
    expect(classifyDiffLine('')).toBe('ctx');
  });
});

describe('provenanceLabel', () => {
  it('collapses an absolute path to its last two segments', () => {
    expect(provenanceLabel('/Users/k/.claude/CLAUDE.md')).toBe('…/.claude/CLAUDE.md');
  });
  it('leaves short paths and empties intact', () => {
    expect(provenanceLabel('CLAUDE.md')).toBe('CLAUDE.md');
    expect(provenanceLabel('')).toBe('unknown target');
  });
});

describe('shortStamp', () => {
  it('formats a parseable RFC3339 and never NaNs an unparseable one', () => {
    expect(shortStamp('2026-06-25T14:32:00Z')).toMatch(/Jun 25 · \d{2}:\d{2}/);
    expect(shortStamp('not-a-date')).toBe('');
    expect(shortStamp('')).toBe('');
  });
});

// ── component fixtures ──────────────────────────────────────────────────────────
function issueFixture(over: Partial<OrbIssue> = {}): OrbIssue {
  return {
    id: 'claude_code:no_delegation',
    agentId: 'claude_code',
    harness: 'claude_code',
    patternId: 'no_delegation',
    title: 'No Delegation',
    count: 3,
    severity: 4,
    rationale: 'Search-heavy turns stayed in the main context.',
    estCostTokens: 1200,
    estCostMinutes: 6,
    frequency: 0.2,
    confidence: 0.8,
    sessionIds: ['sess-abcdef1234'],
    evidence: [],
    findingId: 'finding-1',
    ...over,
  };
}

function issueNode(issue: OrbIssue): LayoutNode {
  return {
    id: issue.id,
    kind: 'issue',
    position: { x: 0, y: 0, z: 0 },
    radius: 1,
    agentId: issue.agentId,
    harness: issue.harness,
    issue,
  };
}

function model(issue: OrbIssue): OrbSceneModel {
  return {
    agents: [
      { id: 'claude_code', harness: 'claude_code', label: 'Claude', glyph: '✶', color: '#3dffa0', sessions: 1, eventCount: 1, totalLoad: 1 },
    ],
    issues: [issue],
    links: [],
    guidance: { doItems: [], stopItems: [] },
  };
}

function scene(): SceneState {
  return { phase: 'idle', candidates: [], verdicts: {}, pulses: [], usage: {}, clustered: 0 };
}

const preview: FixPreview = {
  finding_id: 'finding-1',
  pattern_id: 'no_delegation',
  target_path: '/Users/k/.claude/CLAUDE.md',
  diff: '--- a/CLAUDE.md\n+++ b/CLAUDE.md\n@@ -1 +1,3 @@\n context\n+## WARDEN guardrail — no_delegation\n+Delegate broad search to a subagent.',
  applied: false,
};

function artifactFixture(over: Partial<Artifact> = {}): Artifact {
  return {
    id: 'art-1',
    findingId: 'finding-1',
    kind: 'claude_md_guardrail',
    targetPath: '/Users/k/.claude/CLAUDE.md',
    diff: preview.diff,
    block: '## WARDEN guardrail — no_delegation\nDelegate broad search to a subagent.',
    status: 'pending',
    appliedAt: null,
    backupPath: null,
    preImageSha256: null,
    postImageSha256: null,
    ...over,
  };
}

const noop = () => {};

function renderChrome(props: Partial<React.ComponentProps<typeof Chrome>>): HTMLElement {
  const issue = issueFixture();
  const node = issueNode(issue);
  const full: React.ComponentProps<typeof Chrome> = {
    scene: scene(),
    model: model(issue),
    tab: 'habits',
    hoveredNode: null,
    selectedNode: node,
    focusStack: [],
    running: false,
    error: null,
    fixPreview: preview,
    loadingFix: false,
    artifact: undefined,
    artifacts: [],
    applying: false,
    reverting: false,
    ledgerOpen: false,
    onAsk: noop,
    onRequestFix: noop,
    onApplyFix: noop,
    onRevertFix: noop,
    onToggleLedger: noop,
    onClearSelection: noop,
    onDismiss: noop,
    onPopFocus: noop,
    onClearFocus: noop,
    ...props,
  };
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<Chrome {...full} />));
  return container;
}

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
});

// ── the approval flow ──────────────────────────────────────────────────────────
describe('Chrome — fix approval flow (M4 Forge)', () => {
  it('shows the provenance target + a line-typed diff and an APPLY button when pending', () => {
    const el = renderChrome({ artifact: undefined });
    const target = el.querySelector('[data-fix-target]');
    expect(target?.getAttribute('title')).toBe('/Users/k/.claude/CLAUDE.md');
    expect((target?.textContent ?? '')).toContain('/Users/k/.claude/CLAUDE.md');

    // add-lines carry the add class; the file header carries the file class.
    const adds = el.querySelectorAll('.wd-diff-add');
    expect(adds.length).toBe(2);
    expect(el.querySelectorAll('.wd-diff-file').length).toBe(2);
    expect(el.querySelectorAll('.wd-diff-hunk').length).toBe(1);

    const apply = el.querySelector('[data-fix-apply]') as HTMLButtonElement;
    expect(apply).toBeTruthy();
    expect(apply.disabled).toBe(false);
    expect(apply.textContent).toContain('APPLY GUARDRAIL');
    // no applied badge or revert in the pending state.
    expect(el.querySelector('[data-fix-applied]')).toBeFalsy();
  });

  it('disables APPLY with progress affordance while applying', () => {
    const el = renderChrome({ applying: true });
    const apply = el.querySelector('[data-fix-apply]') as HTMLButtonElement;
    expect(apply.disabled).toBe(true);
    expect(apply.textContent).toContain('APPLYING');
  });

  it('fires onApplyFix with the issue when APPLY is clicked', () => {
    const onApplyFix = vi.fn();
    const el = renderChrome({ onApplyFix });
    const apply = el.querySelector('[data-fix-apply]') as HTMLButtonElement;
    act(() => apply.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(onApplyFix).toHaveBeenCalledTimes(1);
    expect(onApplyFix.mock.calls[0][0].id).toBe('claude_code:no_delegation');
  });

  it('flips to a locked APPLIED badge + REVERT once Artifact.status is applied', () => {
    const applied = artifactFixture({ status: 'applied', appliedAt: '2026-06-25T14:32:00Z', backupPath: '/x/.warden-bak/art-1.bak', preImageSha256: 'deadbeef' });
    const el = renderChrome({ artifact: applied });
    const badge = el.querySelector('[data-applied-badge]');
    expect(badge).toBeTruthy();
    expect((badge?.textContent ?? '')).toContain('GUARDRAIL APPLIED');
    expect((badge?.textContent ?? '')).toMatch(/Jun 25/);
    // Apply CTA is gone; Revert is present.
    expect(el.querySelector('[data-fix-apply]')).toBeFalsy();
    expect(el.querySelector('[data-fix-revert]')).toBeTruthy();
  });

  it('fires onRevertFix with the artifact id from the REVERT button', () => {
    const onRevertFix = vi.fn();
    const applied = artifactFixture({ status: 'applied', appliedAt: '2026-06-25T14:32:00Z' });
    const el = renderChrome({ artifact: applied, onRevertFix });
    const revert = el.querySelector('[data-fix-revert]') as HTMLButtonElement;
    act(() => revert.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(onRevertFix).toHaveBeenCalledWith('art-1');
  });

  it('returns to a (re-apply) candidate state after revert', () => {
    const reverted = artifactFixture({ status: 'reverted', appliedAt: '2026-06-25T14:32:00Z', backupPath: '/x/.warden-bak/art-1.bak' });
    const el = renderChrome({ artifact: reverted });
    expect(el.querySelector('[data-applied-badge]')).toBeFalsy();
    const apply = el.querySelector('[data-fix-apply]') as HTMLButtonElement;
    expect(apply).toBeTruthy();
    expect(apply.textContent).toContain('RE-APPLY GUARDRAIL');
  });

  it('shows ALREADY APPLIED (inert) when the staged diff is empty', () => {
    const empty = artifactFixture({ status: 'pending', diff: '' });
    const el = renderChrome({ artifact: empty });
    const apply = el.querySelector('[data-fix-apply]') as HTMLButtonElement;
    expect(apply.disabled).toBe(true);
    expect(apply.textContent).toContain('ALREADY APPLIED');
  });

  it('keeps the explicit never-writes note in the browser-QA fallback', () => {
    const qaPreview: FixPreview = { ...preview, target_path: 'WARDEN overlay', diff: 'Fix preview is available in the WARDEN app.' };
    const el = renderChrome({ fixPreview: qaPreview, artifact: undefined });
    const note = el.querySelector('[data-fix-note]');
    expect(note).toBeTruthy();
    expect((note?.textContent ?? '').toLowerCase()).toContain('never writes');
    // no apply button writes in QA.
    expect(el.querySelector('[data-fix-apply]')).toBeFalsy();
  });
});

// ── the guardrail ledger ────────────────────────────────────────────────────────
describe('Chrome — guardrail ledger', () => {
  it('counts only historic (applied/reverted) artifacts on the toggle', () => {
    const arts: Artifact[] = [
      artifactFixture({ id: 'a', status: 'applied' }),
      artifactFixture({ id: 'b', status: 'reverted' }),
      artifactFixture({ id: 'c', status: 'pending' }), // not counted
    ];
    const el = renderChrome({ artifacts: arts, ledgerOpen: false });
    const count = el.querySelector('.wd-ledger-toggle-count');
    expect(count?.textContent).toBe('2');
  });

  it('lists applied + reverted rows when open, with a working revert on applied rows', () => {
    const onRevertFix = vi.fn();
    const arts: Artifact[] = [
      artifactFixture({ id: 'a', status: 'applied', targetPath: '/Users/k/.claude/CLAUDE.md' }),
      artifactFixture({ id: 'b', status: 'reverted', targetPath: '/Users/k/.claude/CLAUDE.md' }),
      artifactFixture({ id: 'c', status: 'pending' }),
    ];
    const el = renderChrome({ artifacts: arts, ledgerOpen: true, onRevertFix });
    const rows = Array.from(el.querySelectorAll('[data-ledger-row]')) as HTMLElement[];
    expect(rows).toHaveLength(2); // pending excluded
    expect(rows[0].getAttribute('data-status')).toBe('applied');
    expect(rows[1].getAttribute('data-status')).toBe('reverted');

    // applied row has a revert button; reverted row does not.
    const appliedRevert = rows[0].querySelector('.wd-ledger-revert') as HTMLButtonElement;
    expect(appliedRevert).toBeTruthy();
    expect(rows[1].querySelector('.wd-ledger-revert')).toBeFalsy();

    act(() => appliedRevert.dispatchEvent(new MouseEvent('click', { bubbles: true })));
    expect(onRevertFix).toHaveBeenCalledWith('a');
  });

  it('shows an explicit empty trail when nothing has been written', () => {
    const el = renderChrome({ artifacts: [], ledgerOpen: true });
    expect(el.querySelector('[data-ledger-empty]')).toBeTruthy();
    expect(el.querySelectorAll('[data-ledger-row]')).toHaveLength(0);
  });
});
