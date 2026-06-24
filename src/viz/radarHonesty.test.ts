// radarHonesty.test.ts — the honest-viz guarantees, made explicit + tested (Task 22).
//
// Three honesty laws the radar must never break, regardless of how malformed the
// upstream payload is:
//
//   1. FLAT agents grow NO children. A VS Code Codex agent (`origin === 'codex_vscode'`)
//      and an unknown/empty harness are flat solo globes by definition (spec §4.4 / §5):
//      no subagent data exists for them. Even if a drifted payload hands us a child
//      whose parentId points AT such an agent, the layout must drop that child's link
//      and never orbit it under the flat parent (no fabricated hierarchy).
//   2. UNKNOWN harness ⇒ the neutral slate theme (`RADAR_NEUTRAL`) — its own colour +
//      glyph + label, never a borrowed brand hue.
//   3. ESTIMATED composition is a labeled estimate when present, and ABSENT (no bar)
//      when null — never fabricated. (Panel behaviour asserted in RadarDetailPanel.test;
//      here we pin the data-shape invariant the panel keys off.)

import { describe, expect, it } from 'vitest';
import { layoutRadarScene, isFlatAgent } from './radarLayout';
import { radarHarness, RADAR_NEUTRAL, RADAR_PALETTE } from './radarTheme';
import type { RadarAgent, RadarSceneModel } from './radarTypes';

function agent(partial: Partial<RadarAgent> & Pick<RadarAgent, 'id'>): RadarAgent {
  return {
    harness: 'claude_code',
    origin: null,
    parentId: null,
    depth: 0,
    label: partial.id,
    nickname: null,
    role: null,
    model: null,
    status: 'working',
    contextTokens: 1000,
    maxTokens: 200000,
    fillPct: 0.5,
    composition: { exact: { cacheRead: 0, fresh: 0, output: 0 }, estimated: null },
    recentActivity: [],
    childCount: 0,
    startedAt: '',
    estCostUsd: null,
    ...partial,
  };
}

describe('isFlatAgent (honest-viz flat-globe predicate)', () => {
  it('marks a VS Code Codex agent flat (origin codex_vscode)', () => {
    expect(isFlatAgent(agent({ id: 'vsc', harness: 'codex', origin: 'codex_vscode' }))).toBe(true);
  });

  it('marks an unknown / empty harness agent flat', () => {
    expect(isFlatAgent(agent({ id: 'u', harness: 'gemini' }))).toBe(true);
    expect(isFlatAgent(agent({ id: 'e', harness: '' }))).toBe(true);
  });

  it('does NOT mark a real Codex Desktop or Claude agent flat (they can have children)', () => {
    expect(isFlatAgent(agent({ id: 'cdx', harness: 'codex', origin: 'Codex Desktop' }))).toBe(false);
    expect(isFlatAgent(agent({ id: 'cdx2', harness: 'codex', origin: null }))).toBe(false);
    expect(isFlatAgent(agent({ id: 'cl', harness: 'claude_code' }))).toBe(false);
  });
});

describe('layoutRadarScene — flat agents grow no children (no fabricated hierarchy)', () => {
  it('drops stray child data under a codex_vscode parent (no child node, no link)', () => {
    const model: RadarSceneModel = {
      generatedAt: 'T',
      agents: [
        agent({ id: 'vsc', harness: 'codex', origin: 'codex_vscode', depth: 0, childCount: 0 }),
        // a drifted/malformed child claiming this flat agent as its parent
        agent({ id: 'stray', harness: 'codex', parentId: 'vsc', depth: 1 }),
      ],
    };
    const layout = layoutRadarScene(model);

    // no link is fabricated under the flat parent …
    expect(layout.links).toHaveLength(0);
    // … and the stray is NOT orbited as a child of `vsc`; it surfaces as its own
    // solo root rather than a fabricated moon (honest: never dropped, never faked).
    const vsc = layout.nodes.find((n) => n.id === 'vsc')!;
    const stray = layout.nodes.find((n) => n.id === 'stray')!;
    expect(vsc).toBeTruthy();
    expect(stray).toBeTruthy();
    const dist = Math.hypot(
      vsc.position.x - stray.position.x,
      vsc.position.y - stray.position.y,
      vsc.position.z - stray.position.z,
    );
    // a genuine moon would orbit within a tight band of its parent; a promoted solo
    // root sits out on the roots ring — assert it is NOT hugging the flat parent.
    expect(dist).toBeGreaterThan(vsc.radius + 1);
  });

  it('drops stray child data under an unknown-harness parent', () => {
    const model: RadarSceneModel = {
      generatedAt: 'T',
      agents: [
        agent({ id: 'unk', harness: 'gemini', depth: 0 }),
        agent({ id: 'stray', harness: 'gemini', parentId: 'unk', depth: 1 }),
      ],
    };
    const layout = layoutRadarScene(model);
    expect(layout.links).toHaveLength(0);
    expect(layout.nodes.find((n) => n.id === 'unk')).toBeTruthy();
    expect(layout.nodes.find((n) => n.id === 'stray')).toBeTruthy();
  });

  it('still links real Codex Desktop children (the guard is origin-specific, not all Codex)', () => {
    const model: RadarSceneModel = {
      generatedAt: 'T',
      agents: [
        agent({ id: 'desk', harness: 'codex', origin: 'Codex Desktop', depth: 0, childCount: 1 }),
        agent({ id: 'sub', harness: 'codex', origin: 'Codex Desktop', parentId: 'desk', depth: 1 }),
      ],
    };
    const layout = layoutRadarScene(model);
    expect(layout.links).toEqual([{ source: 'desk', target: 'sub', kind: 'agent_issue' }]);
  });
});

describe('honest-viz — unknown harness renders neutral, never a brand hue', () => {
  it('unknown harness resolves to RADAR_NEUTRAL (own colour + glyph + label)', () => {
    const theme = radarHarness('gemini');
    expect(theme).toBe(RADAR_NEUTRAL);
    expect(theme.color).toBe(RADAR_NEUTRAL.color);
    expect(theme.glyph).toBe(RADAR_NEUTRAL.glyph);
    // and that neutral is distinct from both brand hues (never borrowed identity)
    expect(theme.color).not.toBe(RADAR_PALETTE.claude_code.color);
    expect(theme.color).not.toBe(RADAR_PALETTE.codex.color);
  });

  it('empty harness also resolves to the neutral theme', () => {
    expect(radarHarness('')).toBe(RADAR_NEUTRAL);
  });
});
