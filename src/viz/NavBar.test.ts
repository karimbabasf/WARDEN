import { describe, expect, it } from 'vitest';
import { PRIMARY_VIEW, VIEWS, navItemProps, type NavView } from './NavBar';

// The NavBar component is presentational (verified live in the dev harness, like
// the Scene render). What we CAN honestly assert in node is the pure view model it
// is built from: the two surfaces (RADAR = war room, DOSSIER = profile overlay) and
// the per-item a11y props that drive `aria-current="page"` on exactly the active one.

describe('NavBar view model', () => {
  it('exposes RADAR first (the hero war room), then DOSSIER', () => {
    expect(PRIMARY_VIEW).toBe('radar');
    expect(VIEWS.map((v) => v.id)).toEqual(['radar', 'dossier']);
    expect(VIEWS.map((v) => v.label)).toEqual(['Radar', 'Dossier']);
  });

  it('marks only the active view as the current page (a11y)', () => {
    const active: NavView = 'radar';
    const dossier = navItemProps('dossier', active);
    const radar = navItemProps('radar', active);
    expect(radar['aria-current']).toBe('page');
    expect(radar.active).toBe(true);
    expect(dossier['aria-current']).toBeUndefined();
    expect(dossier.active).toBe(false);
  });

  it('switches the current-page flag with the active view', () => {
    expect(navItemProps('dossier', 'dossier')['aria-current']).toBe('page');
    expect(navItemProps('radar', 'dossier')['aria-current']).toBeUndefined();
  });
});
