// observedTypes.test.ts: `normalizeObservedState` is the one honest seam a peer's frame
// crosses. The load-bearing property is tolerance: a malformed or partial frame degrades
// to an empty/defaulted shape rather than throwing, and nothing here ever manufactures a
// name-shaped field.

import { describe, expect, it } from 'vitest';
import { normalizeObservedState } from './observedTypes';

describe('normalizeObservedState', () => {
  it('normalizes a well-formed frame, including snake_case fallbacks', () => {
    const state = normalizeObservedState({
      generatedAt: '2026-07-26T00:00:00Z',
      truncated: true,
      agents: [
        {
          id: 'a1b2c3d4',
          parent_id: null,
          harness: 'claude_code',
          depth: 0,
          status: 'working',
          project: 'project A',
          role: 'Explore',
          model: 'claude-opus-4-8',
          context_tokens: 1000,
          max_tokens: 200000,
          fill_pct: 0.5,
          context_rows: [{ key: 'messages', tokens: 500, percent_x100: 5000 }],
          child_count: 2,
          age_secs: 120,
          current_action: { kind: 'read', tool: 'Read', elapsed_secs: 4 },
          recent_activity: [{ kind: 'tool', secs_ago: 10 }],
          team: { id: 'team-hash', member_count: 3, is_lead: true, member_type: 'Explore' },
        },
      ],
    });

    expect(state.generatedAt).toBe('2026-07-26T00:00:00Z');
    expect(state.truncated).toBe(true);
    expect(state.agents).toHaveLength(1);
    const a = state.agents[0];
    expect(a.id).toBe('a1b2c3d4');
    expect(a.parentId).toBeNull();
    expect(a.project).toBe('project A');
    expect(a.contextTokens).toBe(1000);
    expect(a.contextRows).toEqual([{ key: 'messages', tokens: 500, percentX100: 5000 }]);
    expect(a.currentAction).toEqual({ kind: 'read', tool: 'Read', elapsedSecs: 4 });
    expect(a.recentActivity).toEqual([{ kind: 'tool', secsAgo: 10 }]);
    expect(a.team).toEqual({ id: 'team-hash', memberCount: 3, isLead: true, memberType: 'Explore' });
  });

  it('degrades a garbage payload to an empty forest instead of throwing', () => {
    expect(() => normalizeObservedState(null)).not.toThrow();
    expect(() => normalizeObservedState(undefined)).not.toThrow();
    expect(() => normalizeObservedState('not an object')).not.toThrow();
    expect(() => normalizeObservedState({ agents: 'not an array' })).not.toThrow();
    const s = normalizeObservedState({ agents: [{ id: 42, contextRows: 'nope', currentAction: 'nope' }] });
    expect(s.agents).toHaveLength(1);
    expect(s.agents[0].id).toBe('');
    expect(s.agents[0].contextRows).toEqual([]);
    expect(s.agents[0].currentAction).toBeNull();
  });

  it('never produces a field capable of carrying a name, label, path or cost', () => {
    const state = normalizeObservedState({
      agents: [{ id: 'x', name: 'Karim', label: 'my-secret-project', cwd: '/Users/karim', estCostUsd: 4.2, prompt: 'do the thing' }],
    });
    const agent = state.agents[0] as unknown as Record<string, unknown>;
    for (const forbidden of ['name', 'label', 'title', 'cwd', 'path', 'prompt', 'estCostUsd', 'cost']) {
      expect(agent).not.toHaveProperty(forbidden);
    }
  });

  it('clamps fillPct to [0, 1] and drops an action missing kind or tool', () => {
    const state = normalizeObservedState({
      agents: [
        { id: 'a', fillPct: 5 },
        { id: 'b', fillPct: -3, currentAction: { kind: 'read' } },
      ],
    });
    expect(state.agents[0].fillPct).toBe(1);
    expect(state.agents[1].fillPct).toBe(0);
    expect(state.agents[1].currentAction).toBeNull();
  });
});
