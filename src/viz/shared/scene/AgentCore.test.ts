import { describe, expect, it } from 'vitest';
import { agentCorePulseState, agentCoreSpinMultiplier } from './AgentCore';

describe('agentCoreSpinMultiplier', () => {
  it('keeps idle gyro rings calm', () => {
    expect(agentCoreSpinMultiplier(false)).toBe(1);
  });

  it('spins working gyro rings just a touch faster', () => {
    const idle = agentCoreSpinMultiplier(false);
    const working = agentCoreSpinMultiplier(true);

    expect(working).toBeGreaterThan(idle);
    expect(working).toBeLessThanOrEqual(1.4);
  });
});

describe('agentCorePulseState', () => {
  it('keeps idle cores steady', () => {
    expect(agentCorePulseState(false, -1)).toEqual({ heartScale: 1, ringScale: 1, glowMultiplier: 1 });
    expect(agentCorePulseState(false, 1)).toEqual({ heartScale: 1, ringScale: 1, glowMultiplier: 1 });
  });

  it('makes working cores breathe in and out with a noticeable but bounded expansion', () => {
    const inward = agentCorePulseState(true, -1);
    const outward = agentCorePulseState(true, 1);

    expect(inward.heartScale).toBeLessThan(1);
    expect(outward.heartScale).toBeGreaterThan(1.1);
    expect(outward.heartScale).toBeLessThanOrEqual(1.12);
    expect(outward.ringScale).toBeGreaterThan(inward.ringScale);
    expect(outward.glowMultiplier).toBeGreaterThan(inward.glowMultiplier);
  });
});
