// devWarRoom.tsx — STATIC war-room QA stage (served at /dev-warroom.html).
//
// Mounts the REAL <WarRoom/> against a bridge pre-loaded once with a
// representative scene — candidates across both harnesses, stage token weight,
// and a few confirmed + one refuted verdict — then holds still (no scripted
// loop, no diagnosis_ready → no reveal overlay). This lets the constellation
// node style be eyeballed/screenshotted without chasing the dev loop's timing.
//
// Complements dev.tsx (the scripted loop); same locked event contract.

import { useEffect } from 'react';
import { createRoot } from 'react-dom/client';
import { createBridge } from '@/viz/shared/state/bridge';
import { WarRoom } from '@/viz/views/war-room/WarRoom';
import REAL_RADAR from './preview/realRadar.json';
import './../style.css';

// listen is unused here (we drive ingest directly) — pass a no-op.
const noopListen = (async () => () => {}) as unknown as Parameters<typeof createBridge>[0];
const bridge = createBridge(noopListen);

const ORB_SCENE = {
  agents: [
    { id: 'claude_code', harness: 'claude_code', label: 'Claude', glyph: '◆', color: '#3dffa0', sessions: 47, event_count: 1402, total_load: 17 },
    { id: 'codex', harness: 'codex', label: 'Codex', glyph: '▣', color: '#b98cff', sessions: 12, event_count: 411, total_load: 8 },
  ],
  issues: [
    { id: 'claude_code:CONTEXT_BLOAT', agent_id: 'claude_code', harness: 'claude_code', pattern_id: 'CONTEXT_BLOAT', title: 'Context bloat', count: 8, severity: 5, rationale: 'Main-context search and file reading recur before useful edits.', est_cost_tokens: 84000, est_cost_minutes: 34, frequency: 0.62, confidence: 0.9, session_ids: ['claude-a', 'claude-b', 'claude-c'], evidence: [] },
    { id: 'claude_code:NO_DELEGATION', agent_id: 'claude_code', harness: 'claude_code', pattern_id: 'NO_DELEGATION', title: 'No delegation', count: 5, severity: 4, rationale: 'Search-heavy sessions stay in the main context instead of delegating discovery.', est_cost_tokens: 44000, est_cost_minutes: 22, frequency: 0.45, confidence: 0.84, session_ids: ['claude-d'], evidence: [] },
    { id: 'claude_code:UNVERIFIED_COMPLETION', agent_id: 'claude_code', harness: 'claude_code', pattern_id: 'UNVERIFIED_COMPLETION', title: 'Unverified completion', count: 4, severity: 5, rationale: 'Substantial tool use ends without an observed verification command.', est_cost_tokens: 20000, est_cost_minutes: 80, frequency: 0.33, confidence: 0.88, session_ids: ['claude-e'], evidence: [] },
    { id: 'codex:CONTEXT_BLOAT', agent_id: 'codex', harness: 'codex', pattern_id: 'CONTEXT_BLOAT', title: 'Context bloat', count: 3, severity: 4, rationale: 'Codex sessions also show repeated main-context discovery, but less persistently.', est_cost_tokens: 21000, est_cost_minutes: 11, frequency: 0.25, confidence: 0.76, session_ids: ['codex-a'], evidence: [] },
    { id: 'codex:WHACK_A_MOLE', agent_id: 'codex', harness: 'codex', pattern_id: 'WHACK_A_MOLE', title: 'Whack-a-mole loops', count: 5, severity: 3, rationale: 'Several sessions show repeated edits around failing commands.', est_cost_tokens: 15000, est_cost_minutes: 35, frequency: 0.42, confidence: 0.74, session_ids: ['codex-b'], evidence: [] },
  ],
  links: [
    { source: 'claude_code', target: 'claude_code:CONTEXT_BLOAT', kind: 'agent_issue' },
    { source: 'claude_code', target: 'claude_code:NO_DELEGATION', kind: 'agent_issue' },
    { source: 'claude_code', target: 'claude_code:UNVERIFIED_COMPLETION', kind: 'agent_issue' },
    { source: 'codex', target: 'codex:CONTEXT_BLOAT', kind: 'agent_issue' },
    { source: 'codex', target: 'codex:WHACK_A_MOLE', kind: 'agent_issue' },
  ],
  guidance: {
    do_items: ['Delegate broad discovery before reading large file sets.'],
    stop_items: ['Stop letting main-context searches accumulate before the first edit.'],
  },
};

function DevApp() {
  useEffect(() => {
    bridge.ingest('orb_scene_ready', ORB_SCENE);
    // Populate the HUD memory profile so the static QA stage looks live.
    bridge.ingest('profile_ready', {
      session_count: 59,
      event_count: 1813,
      finding_count: 25,
      by_harness: [
        { harness: 'claude_code', sessions: 47, events: 1402 },
        { harness: 'codex', sessions: 12, events: 411 },
      ],
    });
    bridge.ingest('radar_scene_ready', REAL_RADAR);
  }, []);

  // forceIntro=false: skip the branded boot, go straight to the constellation.
  return <WarRoom bridge={bridge} forceIntro={false} />;
}

const devRootEl = document.getElementById('dev-root')! as HTMLElement & {
  __wardenWarRoomRoot?: ReturnType<typeof createRoot>;
};
devRootEl.__wardenWarRoomRoot ??= createRoot(devRootEl);
devRootEl.__wardenWarRoomRoot.render(<DevApp />);
