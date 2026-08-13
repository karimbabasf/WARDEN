// warRoomLab.tsx: the WHOLE war-room chrome, served by vite at /war-room-lab.html.
//
// radar-lab.html renders the constellation plus the detail panel in isolation. This
// harness exists for the other half of the problem: the two-rail LAYOUT. It mounts
// the real WarRoom against a mock bridge, so a browser pass can catch the failures
// that only appear when every panel is on screen at once (a rail overlapping the
// scope, the filter dock drifting off the free channel, a label colliding with a
// gauge, a readout that clips at a narrow window).
//
// It feeds the SAME `radar_scene_ready` payload the backend emits, through the real
// `normalizeRadarState`, so the mock can never drift into a shape production does
// not produce. There is no backend: `invoke` rejects and the components fall back
// exactly as they do in the sandbox.
//
// `?watch=1` is the exception, and it is stubbed at the lowest possible level for a
// reason. Watching a peer is the one part of the chrome that is entirely IPC-driven
// (the peer list, the peer's frame, and therefore the watch popover, the second
// constellation and the switcher above them), so with no backend it is unreachable
// in a browser and the layout that state produces could never be checked. The stub
// fills in `window.__TAURI_INTERNALS__`, which is exactly what the real
// `@tauri-apps/api` reads, so the components still run their own untouched invoke
// path and only the answers are fake.

import { createRoot } from 'react-dom/client';
import { createBridge } from '@/viz/shared/state/bridge';
import { WarRoom } from '@/viz/views/war-room/WarRoom';
import '@/style.css';

const NOW = Date.now();
const iso = (secsAgo: number) => new Date(NOW - secsAgo * 1000).toISOString();

function activity(kind: string, label: string, secsAgo: number, target: string | null = null) {
  return { ts: iso(secsAgo), kind, label, target };
}

/**
 * A forest sized like a real busy machine, chosen to exercise every branch the
 * chrome has: a long title that must ellipsize, a session with NO title (falls
 * back to the folder), a near-full context (the hot gauge), an idle agent, a
 * terminated subagent, a team member, and an agent with no cost estimate.
 */
const FOREST = {
  generatedAt: new Date(NOW).toISOString(),
  agents: [
    {
      id: 'a-root-claude',
      harness: 'claude_code',
      parentId: null,
      depth: 0,
      label: 'WARDEN',
      title: 'build frontier website',
      cwd: 'WARDEN',
      model: 'claude-opus-5',
      surface: 'cli',
      status: 'working',
      contextTokens: 172_400,
      maxTokens: 1_000_000,
      fillPct: 0.1724,
      estCostUsd: 4.28,
      childCount: 2,
      startedAt: iso(8040),
      currentAction: {
        kind: 'write',
        tool: 'Edit',
        label: 'Edit agent.rs',
        target: '~/Developer/Apps/WARDEN/src-tauri/src/radar/agent.rs',
        startedAt: iso(42),
        elapsedMs: 42_000,
      },
      composition: { exact: { cacheRead: 148_000, fresh: 19_400, output: 5_000 }, estimated: null },
      recentActivity: [
        activity('write', 'Edit agent.rs', 42, '~/Developer/Apps/WARDEN/src-tauri/src/radar/agent.rs'),
        activity('read', 'Read radarTypes.ts', 96, '~/Developer/Apps/WARDEN/web/viz/shared/types/radarTypes.ts'),
        activity('run', 'cargo test --manifest-path src-tauri/Cargo.toml --lib radar::agent', 150),
        activity('search', 'Grep current_action', 210),
        activity('thinking', 'Considering the termination pass', 260),
      ],
    },
    {
      id: 'a-sub-1',
      harness: 'claude_code',
      parentId: 'a-root-claude',
      depth: 1,
      label: 'subagent 1',
      title: null,
      role: 'Explore',
      model: 'claude-sonnet-5',
      status: 'working',
      contextTokens: 61_200,
      maxTokens: 1_000_000,
      fillPct: 0.0612,
      estCostUsd: 0.41,
      childCount: 0,
      startedAt: iso(900),
      currentAction: { kind: 'read', tool: 'Read', label: 'Read store.rs', target: '~/Developer/Apps/WARDEN/src-tauri/src/store.rs', startedAt: iso(9), elapsedMs: 9_000 },
      team: { id: 't1', name: 'radar-team', memberName: 'BackendMap', memberType: 'Explore', memberCount: 3, isLead: false },
      composition: { exact: { cacheRead: 52_000, fresh: 8_200, output: 1_000 }, estimated: null },
      recentActivity: [activity('read', 'Read store.rs', 9, '~/Developer/Apps/WARDEN/src-tauri/src/store.rs')],
    },
    {
      id: 'a-sub-2',
      harness: 'claude_code',
      parentId: 'a-root-claude',
      depth: 1,
      label: 'subagent 2',
      role: 'general-purpose',
      model: 'claude-haiku-4-5-20251001',
      status: 'terminated',
      contextTokens: 24_000,
      maxTokens: 200_000,
      fillPct: 0.12,
      estCostUsd: 0.03,
      childCount: 0,
      startedAt: iso(1800),
      composition: { exact: { cacheRead: 20_000, fresh: 3_000, output: 1_000 }, estimated: null },
      recentActivity: [activity('tool', 'Task complete', 300)],
    },
    {
      id: 'a-root-codex',
      harness: 'codex',
      parentId: null,
      depth: 0,
      label: 'pakkr',
      title: 'wire the switchboard call summary into Telegram topics',
      cwd: 'pakkr',
      model: 'gpt-5.6-sol',
      surface: 'codex_vscode',
      status: 'working',
      contextTokens: 236_800,
      maxTokens: 272_000,
      fillPct: 0.87,
      estCostUsd: 11.4,
      childCount: 0,
      startedAt: iso(15300),
      currentAction: { kind: 'run', tool: 'exec_command', label: 'pnpm vitest run src/switchboard', target: null, startedAt: iso(310), elapsedMs: 310_000 },
      composition: { exact: { cacheRead: 201_000, fresh: 28_800, output: 7_000 }, estimated: null },
      recentActivity: [
        activity('run', 'pnpm vitest run src/switchboard', 310),
        activity('write', 'apply_patch summary.ts', 420, '~/Developer/Apps/Pakkr/src/switchboard/summary.ts'),
        activity('read', "sed -n '1,240p' telnyx.ts", 480, '~/Developer/Apps/Pakkr/src/switchboard/telnyx.ts'),
      ],
    },
    {
      id: 'a-root-idle',
      harness: 'claude_code',
      parentId: null,
      depth: 0,
      label: 'karim-tracker',
      title: null,
      cwd: 'karim-tracker',
      model: 'claude-fable-5',
      surface: 'claude-vscode',
      status: 'idle',
      contextTokens: 8_400,
      maxTokens: 1_000_000,
      fillPct: 0.0084,
      estCostUsd: null,
      childCount: 0,
      startedAt: iso(300),
      composition: { exact: { cacheRead: 6_000, fresh: 2_000, output: 400 }, estimated: null },
      recentActivity: [activity('message', 'Nothing outstanding', 120)],
    },
    {
      // The third state, in the rack. This lab is the LAYOUT pass, so what it has to
      // prove is that a waiting strip is findable in a full rack without being louder
      // than the board: the rule strobes, the word says Waiting, and the action row says
      // what it wants instead of "No action in flight".
      id: 'a-root-awaiting',
      harness: 'claude_code',
      parentId: null,
      depth: 0,
      label: 'frontier-site',
      title: 'price the paid edition',
      cwd: 'frontier-site',
      model: 'claude-opus-5',
      surface: 'cli',
      status: 'awaiting',
      awaitingReason: 'question',
      contextTokens: 96_500,
      maxTokens: 1_000_000,
      fillPct: 0.0965,
      estCostUsd: 1.62,
      childCount: 0,
      startedAt: iso(2600),
      currentAction: {
        kind: 'ask',
        tool: 'AskUserQuestion',
        label: 'Hold the price at $2.50 an edition, or move it to $3?',
        target: null,
        startedAt: iso(74),
        elapsedMs: 74_000,
      },
      composition: { exact: { cacheRead: 80_000, fresh: 14_500, output: 2_000 }, estimated: null },
      recentActivity: [activity('ask', 'AskUserQuestion', 74)],
    },
    {
      // A second waiting agent, so the lab shows the two flashing IN SYNC. One alarm,
      // not two twitches: that is the reason the strobe is not phase-scattered.
      id: 'a-root-awaiting-2',
      harness: 'codex',
      parentId: null,
      depth: 0,
      label: 'ledger-svc',
      title: null,
      cwd: 'ledger-svc',
      model: 'gpt-5.6-sol',
      surface: 'codex_vscode',
      status: 'awaiting',
      awaitingReason: 'approval',
      contextTokens: 44_000,
      maxTokens: 272_000,
      fillPct: 0.162,
      estCostUsd: 0.51,
      childCount: 0,
      startedAt: iso(1100),
      composition: { exact: { cacheRead: 36_000, fresh: 6_500, output: 1_500 }, estimated: null },
      recentActivity: [activity('message', 'Ready to run both migrations.', 210)],
    },
  ],
};

// `createBridge` takes the Tauri event listener so the bridge can self-wire in the
// app. There is no Tauri here, and this harness drives the bridge directly through
// `ingest`, so a no-op stub returning an unsubscribe is the whole dependency.
const noopListen = (async () => () => {}) as unknown as typeof import('@tauri-apps/api/event').listen;
const bridge = createBridge(noopListen);
const el = document.getElementById('war-room-root');
const params = new URLSearchParams(window.location.search);

// A peer's board, in the redacted `ObservedState` shape the projection really emits:
// no paths, no titles, no cwd. Deliberately a different size and mix from the local
// forest so the two constellations are told apart at a glance in a screenshot.
const PEER_STATE = {
  generatedAt: new Date(NOW).toISOString(),
  truncated: false,
  agents: [
    { id: 'p1', harness: 'codex', parentId: null, depth: 0, status: 'working', fillPct: 0.44, childCount: 1 },
    { id: 'p2', harness: 'codex', parentId: 'p1', depth: 1, status: 'working', fillPct: 0.12, childCount: 0 },
    { id: 'p3', harness: 'claude_code', parentId: null, depth: 0, status: 'idle', fillPct: 0.03, childCount: 0 },
  ],
};

const PEER_ROW = {
  peerId: 'peer-1',
  hostLabel: 'Warden host',
  fingerprint: '1a49-1089-43ad-4e6e-11a7-a61b',
  connected: true,
  lastFrameAt: new Date(NOW - 4000).toISOString(),
  error: null,
};

// `?watch=1` makes the observer flows reachable without a backend: a peer already
// added and already sending frames. Installed BEFORE mount so the first
// `observe_list_peers` of the session already resolves.
if (params.get('watch') === '1') {
  // `?peer=silent` is a peer that has been added and selected but has sent nothing
  // yet, and `?peer=error` is one whose connection failed. Both are states the
  // observer path really produces and neither was reachable in a browser before.
  const peerMode = params.get('peer') ?? 'live';
  const answers: Record<string, unknown> = {
    observe_list_peers: [
      peerMode === 'error'
        ? { ...PEER_ROW, connected: false, lastFrameAt: null, error: 'could not reach that machine' }
        : peerMode === 'silent'
          ? { ...PEER_ROW, connected: true, lastFrameAt: null }
          : PEER_ROW,
    ],
    observe_peer_state: peerMode === 'live' ? PEER_STATE : null,
  };
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
    invoke: (cmd: string) =>
      cmd in answers ? Promise.resolve(answers[cmd]) : Promise.reject(new Error(`no stub for ${cmd}`)),
    transformCallback: (cb: unknown) => cb,
  };
}

// `?solo=1` drops every non-Claude agent. A one-harness machine is the common case
// and it is a DIFFERENT chrome (the filter dock does not render at all when there is
// nothing to choose between), so it needs its own reachable state.
// `?empty=1` is the cold machine: the radar is live and watching, nothing is running.
// It is an honest state rather than a failure, so it has to be looked at like one.
const board = params.get('empty') === '1'
  ? { ...FOREST, agents: [] }
  : params.get('solo') === '1'
    ? { ...FOREST, agents: FOREST.agents.filter((a) => a.harness === 'claude_code') }
    : FOREST;

if (el) {
  createRoot(el).render(<WarRoom bridge={bridge} />);
  // Push after mount so the harness exercises the real subscribe path rather than
  // rendering from a pre-seeded state the app would never actually start from.
  bridge.ingest('radar_scene_ready', board);
  // The app pulls `get_radar_state` on an interval and swallows the rejection when
  // there is no Tauri backend, so re-push on the same cadence to keep the mock in
  // place instead of letting the first failed pull look like an empty machine.
  window.setInterval(() => bridge.ingest('radar_scene_ready', board), 750);

  // `?select=<n>` clicks the nth fleet strip once the rail has rendered. The
  // both-rails-open state is the one that actually has to be checked for overlap
  // (left rack, scope, right readout, filter dock, breadcrumb all on screen at
  // once), and it is unreachable in a static screenshot without a click.
  const want = params.get('select');
  if (want !== null) {
    const n = Number(want) || 0;
    window.setTimeout(() => {
      const strips = document.querySelectorAll<HTMLButtonElement>('.wd-strip');
      strips[n]?.click();
    }, 400);
  }

  // `?fold=1` folds the rack down to its tab, `?fold=0` opens it. Driven through the
  // real control rather than the storage key, so the harness cannot drift from the
  // app. Stated EXPLICITLY on both sides because the fold persists: without naming
  // the state you want, a screenshot run inherits whatever the last one left behind.
  const fold = params.get('fold');
  if (fold !== null) {
    window.setTimeout(() => {
      const folded = document.querySelector('.wd-fleet.is-folded') !== null;
      if (folded === (fold === '1')) return;
      document.querySelector<HTMLButtonElement>(fold === '1' ? '.wd-fleet-fold' : '.wd-fleet-tab')?.click();
    }, 450);
  }

  // With `?watch=1`, `?dock=open` holds the watch popover open over the rail (the
  // state that used to tear into the fleet strips) and `?dock=peer` picks the peer,
  // which closes the popover and leaves the two constellations plus the switcher.
  // Both are unreachable in a static screenshot without the click.
  if (params.get('watch') === '1') {
    const dock = params.get('dock') ?? 'peer';
    window.setTimeout(() => {
      document.querySelector<HTMLButtonElement>('.wd-observe-watch-trigger')?.click();
      if (dock === 'peer') {
        window.setTimeout(() => {
          document.querySelector<HTMLButtonElement>('.wd-observe-peer-btn')?.click();
          // `?view=local` then switches back to your own board, which is the state
          // that was unreachable before the switcher existed and therefore the one
          // most worth being able to screenshot.
          if (params.get('view') === 'local') {
            window.setTimeout(
              () => document.querySelectorAll<HTMLButtonElement>('.wd-constellation-seg')[0]?.click(),
              300,
            );
          }
        }, 250);
      }
    }, 500);
  }
}
