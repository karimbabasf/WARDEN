// dossierPreview.tsx — browser QA harness for the rebuilt DOSSIER.
//
// Renders the REAL ProfileScreen (via its own openProfile() mount path) against
// a mocked Tauri v2 backend, so the populated dossier can be seen in a plain
// browser (no `pnpm tauri dev` / native window needed). Same spirit as
// radarReal.tsx. Mock data is realistic but synthetic — QA only, never shipped
// to users. The mock __TAURI_INTERNALS__ is installed BEFORE mount.tsx loads
// (dynamic import) so the real `invoke`/`listen` route through it.

import '../../style.css'; // phosphor :root tokens the scoped dossier.css references

const ev = (
  session_id: string,
  quote: string,
  turn_id: string | null = 'turn-4',
  source_path: string | null = '/Users/karimbaba/WARDEN/src-tauri/src/store.rs',
) => ({ session_id, turn_id, event_id: 'evt-1', quote, source_path });

const MOCK_PROFILE = {
  window: 'all-time',
  generated_at: '2026-07-04T18:00:00Z',
  data_hash: 'a1b2c3d4e5f6',
  rubric_version: 'v3',
  detector_only: false,
  session_count: 214,
  efficiency: {
    headline: 0.62,
    rubric_version: 'v3',
    session_count: 214,
    families: [
      { key: 'parallel_delegation', sub_score: 0.86, weight: 0.18 },
      { key: 'verification_discipline', sub_score: 0.79, weight: 0.18 },
      { key: 'planning_before_build', sub_score: 0.68, weight: 0.14 },
      { key: 'tooling_leverage', sub_score: 0.64, weight: 0.12 },
      { key: 'scope_control', sub_score: 0.51, weight: 0.12 },
      { key: 'context_hygiene', sub_score: 0.33, weight: 0.14 },
      { key: 'handoff_verification', sub_score: 0.28, weight: 0.12 },
    ],
  },
  dimensions: [
    {
      key: 'orchestration_style',
      title: 'Orchestration style',
      narrative: 'You run a fan-out orchestrator: bounded subagents in parallel, discovery pushed to cheap models, state kept on disk.',
      claims: [
        { text: 'Delegates discovery to parallel subagents rather than exploring in the main context', confidence: 0.88, status: 'asserted', evidence: [ev('sess-01', 'dispatched four Explore agents in one message'), ev('sess-02', 'kept only the distilled conclusion, not the file dumps', 'turn-9')] },
      ],
    },
    {
      key: 'signature_patterns',
      title: 'Signature patterns',
      narrative: 'Spec → phased plan → build → verify. You commit frequently and gate on real build/test output.',
      claims: [
        { text: 'Writes a spec and a phased plan before touching code', confidence: 0.8, status: 'asserted', evidence: [ev('sess-03', 'committed the design spec before implementation', 'turn-2')] },
      ],
    },
    {
      key: 'strengths',
      title: 'Strengths',
      narrative: 'Verification-first, and genuinely parallel. You rarely claim done without evidence.',
      claims: [
        { text: 'Verifies against ground truth before claiming completion', confidence: 0.83, status: 'asserted', evidence: [ev('sess-04', 're-ran the full test suite before committing', 'turn-7'), ev('sess-05', 'checked git log to confirm the commits were real', 'turn-3'), ev('sess-06', 'confirmed build output was green', 'turn-9')] },
        { text: 'Runs independent work concurrently', confidence: 0.77, status: 'asserted', evidence: [ev('sess-07', 'launched B and C builders back to back')] },
      ],
    },
    {
      key: 'holes',
      title: 'Holes',
      narrative: 'Context hygiene lags: long-lived sessions accrete stale files, and handoffs to subagents sometimes go unverified.',
      claims: [
        { text: 'Lets context windows fill past the point of reliable recall', confidence: 0.46, status: 'emerging', evidence: [ev('sess-08', 'continued a complex task past ~70% context', 'turn-14')] },
        { text: 'Occasionally trusts a subagent report without checking git', confidence: 0.41, status: 'emerging', evidence: [ev('sess-09', 'accepted "done" before a stall was caught', 'turn-5')] },
      ],
    },
    {
      key: 'where_you_lose',
      title: 'Where you lose',
      narrative: 'Most wasted spend traces to two places: re-running failing commands unchanged, and un-scoped searches that flood context.',
      claims: [
        { text: 'Retries a failing command without changing approach', confidence: 0.58, status: 'emerging', evidence: [ev('sess-10', 'retried cargo build a third time unchanged', 'turn-2')] },
      ],
    },
    {
      key: 'project_archetypes',
      title: 'Project archetypes',
      narrative: 'You move between greenfield tool-building and deep debugging, with occasional large refactors.',
      claims: [
        { text: 'Spends most sessions building agent-observability tooling', confidence: 0.72, status: 'asserted', evidence: [ev('sess-11', 'WARDEN daemon + war-room across dozens of sessions')] },
      ],
    },
    {
      key: 'trajectory',
      title: 'Trajectory',
      narrative: 'Outcome quality is trending up as verification discipline solidifies.',
      claims: [
        { text: 'Fewer unverified completions over the last month', confidence: 0.55, status: 'emerging', evidence: [ev('sess-12', 'verification-before-completion applied consistently')] },
      ],
    },
  ],
  ranked_leaks: [
    { rank: 1, title: 'Re-runs the same failing command without changing approach', est_cost_tokens: 41200, est_cost_minutes: 34, evidence: [ev('sess-10', 'retried cargo build a third time unchanged', 'turn-2'), ev('sess-13', 'same error, same command, no diagnosis', 'turn-3')] },
    { rank: 2, title: 'Un-scoped searches that flood the context window', est_cost_tokens: 28800, est_cost_minutes: 22, evidence: [ev('sess-14', 'grep -r across the whole tree, 56KB into context')] },
    { rank: 3, title: 'Unverified handoffs to subagents', est_cost_tokens: 19500, est_cost_minutes: 18, evidence: [ev('sess-09', 'accepted a subagent "done" that had stalled', 'turn-5')] },
    { rank: 4, title: 'Long-lived sessions past the context-rot threshold', est_cost_tokens: 15300, est_cost_minutes: 12, evidence: [ev('sess-08', 'complex task begun past ~70% context', 'turn-14')] },
    { rank: 5, title: 'Re-reading files already held after an edit', est_cost_tokens: 9100, est_cost_minutes: 7, evidence: [ev('sess-15', 'read a file back right after editing it')] },
  ],
  archetypes: [
    { archetype: 'Greenfield tooling', projects: ['WARDEN', 'context-mode', 'rtk'], session_count: 96, note: 'Building agent-observability and dev-tooling from scratch.' },
    { archetype: 'Deep debugging', projects: ['WARDEN radar', 'brain pipeline'], session_count: 71, note: 'Systematic root-cause work on live-data and reasoning bugs.' },
    { archetype: 'Large refactor', projects: ['dossier-merge'], session_count: 47, note: 'Multi-subsystem restructures under test coverage.' },
  ],
  trajectory: [
    { trait_key: 'outcome', direction: 'improving', confidence: 0.55, points: [
      { bucket: '2026-04', value: 0.48 }, { bucket: '2026-05', value: 0.55 }, { bucket: '2026-06', value: 0.62 }, { bucket: '2026-07', value: 0.66 },
    ] },
  ],
};

// A ~17-week activity heatmap with plausible variation.
const MOCK_HEATMAP = (() => {
  const cells: any[] = [];
  const start = new Date('2026-03-08T00:00:00Z');
  for (let i = 0; i < 119; i++) {
    const d = new Date(start.getTime() + i * 86400000);
    const date = d.toISOString().slice(0, 10);
    const dow = d.getUTCDay();
    const base = dow === 0 || dow === 6 ? 0.35 : 1;
    const wave = 0.5 + 0.5 * Math.sin(i / 6);
    const tokens = Math.round(base * wave * (120000 + (i % 11) * 24000));
    const sessions = tokens > 20000 ? 1 + (i % 4) : tokens > 0 ? 1 : 0;
    cells.push({
      date,
      total_tokens: tokens,
      session_count: sessions,
      by_harness: [
        { harness: 'claude_code', tokens: Math.round(tokens * 0.72), sessions: Math.max(0, sessions - 1) },
        { harness: 'codex', tokens: Math.round(tokens * 0.28), sessions: sessions > 1 ? 1 : 0 },
      ],
    });
  }
  return cells;
})();

// habits_refreshed payload — three anti-patterns in remediation (snake_case OrbIssue).
const MOCK_HABITS = {
  window: 'all',
  last_scanned_at: '2026-07-04T18:02:00Z',
  issues: [
    { id: 'h1', pattern_id: 'context_flooding', title: 'Un-scoped searches that flood context', est_cost_tokens: 28800, est_cost_minutes: 22, severity: 3, credits: 7, streak_k: 8, fixed: false, last_credit_at: '2026-07-01T10:00:00Z', evidence: [ev('sess-14', 'grep -r across the whole tree')] },
    { id: 'h2', pattern_id: 'blind_retry', title: 'Re-runs a failing command unchanged', est_cost_tokens: 41200, est_cost_minutes: 34, severity: 4, credits: 2, streak_k: 8, fixed: false, last_credit_at: '2026-06-28T14:00:00Z', evidence: [ev('sess-10', 'retried cargo build a third time unchanged', 'turn-2')] },
    { id: 'h3', pattern_id: 'unverified_handoff', title: 'Unverified handoffs to subagents', est_cost_tokens: 19500, est_cost_minutes: 18, severity: 3, credits: 8, streak_k: 8, fixed: true, last_credit_at: '2026-07-03T09:00:00Z', evidence: [ev('sess-09', 'now checks git after every subagent', 'turn-5')] },
  ],
};

const cbs: Record<number, (e: any) => void> = {};
let idc = 0;
(window as any).__TAURI_INTERNALS__ = {
  transformCallback(cb: (e: any) => void) {
    const id = ++idc;
    cbs[id] = cb;
    return id;
  },
  invoke(cmd: string, args: any) {
    switch (cmd) {
      case 'plugin:event|listen': {
        if (args?.event === 'habits_refreshed') {
          const h = args.handler;
          setTimeout(() => cbs[h]?.({ event: 'habits_refreshed', id: h, payload: MOCK_HABITS }), 90);
        }
        return Promise.resolve(++idc);
      }
      case 'plugin:event|unlisten':
        return Promise.resolve();
      case 'get_profile':
        return Promise.resolve(MOCK_PROFILE);
      case 'get_efficiency_score':
        return Promise.resolve(MOCK_PROFILE.efficiency);
      case 'get_activity_heatmap':
        return Promise.resolve(MOCK_HEATMAP);
      case 'set_habits_window':
        return Promise.resolve(null);
      case 'resolve_evidence':
        return Promise.resolve({ session_id: args?.sessionId ?? 'sess', turn_id: 'turn-7', quote: 'resolved evidence context', context: [] });
      default:
        return Promise.resolve(null);
    }
  },
};

// Mount the REAL dossier now that the mock backend exists.
const { openProfile } = await import('../profile/mount');
openProfile();
