import { describe, expect, it } from 'vitest';
import { TABS, navItemProps, type ConstellationTab } from './NavBar';

// The NavBar component is presentational (verified live in the dev harness, like
// the Scene render). What we CAN honestly assert in node is the pure tab model it
// is built from: the two constellation tabs and the per-item a11y props that drive
// `aria-current="page"` on exactly the active tab.

describe('NavBar tab model', () => {
  it('exposes exactly the Habits and Radar constellations, in order', () => {
    expect(TABS.map((t) => t.id)).toEqual(['habits', 'radar']);
    expect(TABS.map((t) => t.label)).toEqual(['Habits', 'Radar']);
  });

  it('marks only the active tab as the current page (a11y)', () => {
    const active: ConstellationTab = 'radar';
    const habits = navItemProps('habits', active);
    const radar = navItemProps('radar', active);
    expect(radar['aria-current']).toBe('page');
    expect(radar.active).toBe(true);
    expect(habits['aria-current']).toBeUndefined();
    expect(habits.active).toBe(false);
  });

  it('switches the current-page flag with the active tab', () => {
    expect(navItemProps('habits', 'habits')['aria-current']).toBe('page');
    expect(navItemProps('radar', 'habits')['aria-current']).toBeUndefined();
  });
});
