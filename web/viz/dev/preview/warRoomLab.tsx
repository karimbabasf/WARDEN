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
      recentActivity: [activity('message', 'Waiting on you', 120)],
    },
  ],
};

// `createBridge` takes the Tauri event listener so the bridge can self-wire in the
// app. There is no Tauri here, and this harness drives the bridge directly through
// `ingest`, so a no-op stub returning an unsubscribe is the whole dependency.
const noopListen = (async () => () => {}) as unknown as typeof import('@tauri-apps/api/event').listen;
const bridge = createBridge(noopListen);
const el = document.getElementById('war-room-root');
if (el) {
  createRoot(el).render(<WarRoom bridge={bridge} />);
  // Push after mount so the harness exercises the real subscribe path rather than
  // rendering from a pre-seeded state the app would never actually start from.
  bridge.ingest('radar_scene_ready', FOREST);
  // The app pulls `get_radar_state` on an interval and swallows the rejection when
  // there is no Tauri backend, so re-push on the same cadence to keep the mock in
  // place instead of letting the first failed pull look like an empty machine.
  window.setInterval(() => bridge.ingest('radar_scene_ready', FOREST), 750);

  // `?select=<n>` clicks the nth fleet strip once the rail has rendered. The
  // both-rails-open state is the one that actually has to be checked for overlap
  // (left rack, scope, right readout, filter dock, breadcrumb all on screen at
  // once), and it is unreachable in a static screenshot without a click.
  const want = new URLSearchParams(window.location.search).get('select');
  if (want !== null) {
    const n = Number(want) || 0;
    window.setTimeout(() => {
      const strips = document.querySelectorAll<HTMLButtonElement>('.wd-strip');
      strips[n]?.click();
    }, 400);
  }
}
