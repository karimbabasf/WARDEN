// @vitest-environment jsdom

import { describe, expect, it } from 'vitest';
import { autoDismisses, HUD_AUTO_LINGER_MS } from './HudRoot';

describe('autoDismisses', () => {
  it('leaves on its own only when nobody asked for the panel', () => {
    expect(autoDismisses(true, false, 'open')).toBe(true);
    expect(autoDismisses(false, false, 'open')).toBe(false);
  });

  it('stops leaving the moment the operator reaches for it', () => {
    // Engaging is what turns a notification into the panel a click would have opened:
    // it must not vanish out from under a pointer that is on its way to a globe.
    expect(autoDismisses(true, true, 'open')).toBe(false);
    expect(autoDismisses(true, true, 'opening')).toBe(false);
  });

  it('runs while the panel is arriving, not only once it has settled', () => {
    // The clock has to cover `opening` too, or a summon whose spring is still playing
    // would start its linger late and sit there longer than the one before it.
    expect(autoDismisses(true, false, 'opening')).toBe(true);
  });

  it('never runs on a panel that is closed or on its way out', () => {
    expect(autoDismisses(true, false, 'closed')).toBe(false);
    expect(autoDismisses(true, false, 'closing')).toBe(false);
  });

  it('lingers long enough to read a fleet and short enough to ignore', () => {
    expect(HUD_AUTO_LINGER_MS).toBeGreaterThanOrEqual(3000);
    expect(HUD_AUTO_LINGER_MS).toBeLessThanOrEqual(10000);
  });
});
