// brickyardLab.tsx: the Brickyard against simulated agents, served by vite at
// /brickyard-lab.html. The mock `fetchLog` grows each agent's build log over time
// with the same shape the Rust command returns, so the planner, pacer and rigs run
// exactly as they do on real sessions.
//
//   ?pre=<n>     start the lead of bench 0 with n entries already logged
//   ?speed=<x>   simulated agents work x times faster (default 1)
//   ?select=<i>  open bench i's panel
//   ?burst=1     bench 0 fires 60 calls at once, to watch the catch-up
//   ?real=<file> bench 0 replays a real build log dumped by `build_log_probe --json`
//                into public/brickyard/ (dev only, never committed)

import { useEffect, useMemo, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { BrickyardView, type FetchLog } from '@/viz/modules/brickyard/BrickyardView';
import { normalizeBuildLog, type BuildAction, type BuildStep } from '@/viz/modules/brickyard/buildLog';
import { normalizeRadarState, type RadarAgent } from '@/viz/shared/types/radarTypes';
import '@/style.css';

const params = new URLSearchParams(location.search);
const SPEED = Number(params.get('speed') ?? '1') || 1;
const PRE = Number(params.get('pre') ?? '0') || 0;
const NOW = Date.now();
const iso = (ms: number) => new Date(ms).toISOString();

const TOOLS: Record<BuildAction, { tool: string; label: string; target: string | null }[]> = {
  place: [
    { tool: 'Edit', label: 'Edit planner.ts', target: '~/Developer/Apps/WARDEN/web/viz/modules/brickyard/planner.ts' },
    { tool: 'Write', label: 'Write bench.ts', target: '~/Developer/Apps/WARDEN/web/viz/modules/brickyard/bench.ts' },
    { tool: 'Bash', label: 'pnpm vitest run', target: null },
    { tool: 'Edit', label: 'Edit build_log.rs', target: '~/Developer/Apps/WARDEN/src-tauri/src/radar/build_log.rs' },
  ],
  browse: [
    { tool: 'Read', label: 'Read WarRoom.tsx', target: '~/Developer/Apps/WARDEN/web/viz/views/war-room/WarRoom.tsx' },
    { tool: 'Grep', label: 'Grep buildingStep', target: null },
  ],
  page: [{ tool: 'TodoWrite', label: 'TodoWrite (plan)', target: null }],
  pull: [{ tool: 'Bash', label: 'exit code 1: 2 tests failed', target: null }],
  spawn: [{ tool: 'Agent', label: 'Agent map the repo', target: null }],
  ask: [{ tool: 'AskUserQuestion', label: 'AskUserQuestion', target: null }],
};

/** A deterministic little agent: what it does at each call, and when. */
class SimAgent {
  log: BuildStep[] = [];
  private placed: number[] = [];
  private t: number;
  private rnd: number;
  constructor(readonly id: string, seed: number, readonly rate: number, pre = 0) {
    this.rnd = seed;
    this.t = NOW - pre * 900;
    for (let i = 0; i < pre; i++) this.next(this.t + i * 900);
  }
  private rand() {
    this.rnd = (this.rnd * 1103515245 + 12345) % 2147483648;
    return this.rnd / 2147483648;
  }
  private next(at: number, force?: BuildAction) {
    const r = this.rand();
    let action: BuildAction = force ?? (r < 0.66 ? 'place' : r < 0.86 ? 'browse' : r < 0.93 ? 'page' : 'pull');
    if (action === 'pull' && this.placed.length === 0) action = 'place';
    const pool = TOOLS[action];
    const pick = pool[Math.floor(this.rand() * pool.length)];
    const seq = this.log.length;
    let undoes: number | null = null;
    if (action === 'pull') undoes = this.placed.splice(Math.floor(this.rand() * this.placed.length), 1)[0];
    if (action === 'place') this.placed.push(seq);
    this.log.push({ seq, ts: iso(at), action, tool: pick.tool, label: pick.label, target: pick.target, undoes });
  }
  /** Catch the log up to wall-clock time `now`. */
  advance(now: number) {
    const gap = 1000 / (this.rate * SPEED);
    while (this.t + gap <= now) {
      this.t += gap * (0.4 + this.rand() * 1.2);
      this.next(this.t);
    }
  }
  burst(n: number) {
    for (let i = 0; i < n; i++) this.next(Date.now(), 'place');
  }
}

const sims = new Map<string, SimAgent>([
  ['lead-a', new SimAgent('lead-a', 7, 1.1, PRE)],
  ['helper-a1', new SimAgent('helper-a1', 11, 0.9)],
  ['lead-b', new SimAgent('lead-b', 23, 0.7, Math.floor(PRE / 2))],
  ['lead-c', new SimAgent('lead-c', 31, 0.0, 12)],
]);
if (params.get('burst') === '1') sims.get('lead-a')?.burst(60);

const REAL = params.get('real');
const realLog: Promise<BuildStep[]> | null = REAL
  ? fetch(`/brickyard/${REAL}`).then((r) => r.json()).then((x) => normalizeBuildLog(x))
  : null;

const fetchLog: FetchLog = async (agentId, after) => {
  if (realLog && agentId === 'lead-a') return (await realLog).filter((s) => after === null || s.seq > after);
  const sim = sims.get(agentId);
  if (!sim) return [];
  sim.advance(Date.now());
  return sim.log.filter((s) => after === null || s.seq > after);
};

function agent(id: string, over: Partial<RadarAgent> & Record<string, unknown>) {
  return {
    id,
    harness: 'claude_code',
    origin: null,
    parentId: null,
    depth: 0,
    label: id,
    nickname: null,
    cwd: 'WARDEN',
    repo: null,
    role: null,
    model: 'claude-opus-5-5',
    status: 'working',
    contextTokens: 80000,
    maxTokens: 200000,
    fillPct: 0.4,
    composition: { exact: { cacheRead: 0, fresh: 0, cacheWrite: 0, output: 0 }, estimated: null },
    recentActivity: [],
    childCount: 0,
    startedAt: iso(NOW - 600_000),
    estCostUsd: null,
    ...over,
  };
}

function Lab() {
  const [helperDone, setHelperDone] = useState(false);
  useEffect(() => {
    const t = window.setTimeout(() => setHelperDone(true), 45_000 / SPEED);
    return () => window.clearTimeout(t);
  }, []);
  const agents = useMemo(
    () =>
      normalizeRadarState({
        generatedAt: iso(NOW),
        agents: [
          agent('lead-a', { label: 'Build the Brickyard view', childCount: 1 }),
          agent('helper-a1', {
            parentId: 'lead-a',
            depth: 1,
            label: 'map the repo',
            status: helperDone ? 'closed' : 'working',
            startedAt: iso(NOW - 300_000),
          }),
          agent('lead-b', { harness: 'codex', label: 'phosphor: fix the swap quote', cwd: 'phosphor', startedAt: iso(NOW - 500_000) }),
          agent('lead-c', { label: 'site: checkout copy', status: 'awaiting', awaitingReason: 'question', startedAt: iso(NOW - 400_000) }),
        ],
      }).agents,
    [helperDone],
  );
  const sel = params.get('select');
  const [selectedId, setSelectedId] = useState<string | null>(sel !== null ? (['lead-a', 'lead-b', 'lead-c'][Number(sel)] ?? null) : null);
  return (
    <div className="viz-root wd-radar-root">
      <BrickyardView agents={agents} frameloop="always" active selectedId={selectedId} onSelect={setSelectedId} fetchLog={fetchLog} />
    </div>
  );
}

createRoot(document.getElementById('war-room-root') as HTMLElement).render(<Lab />);
