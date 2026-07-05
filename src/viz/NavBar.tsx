// NavBar.tsx — the top view switch (RADAR | DOSSIER).
//
// WARDEN is refocused around RADAR, the live 3D agent-fleet map, as the hero. The
// war room now renders RADAR only; this bar is the single place to move between
// the two surfaces the operator lives in:
//   • RADAR   → the war room itself (the live fleet). Selecting it closes the
//               DOSSIER overlay so the fleet shows through.
//   • DOSSIER → the full-page "Profile by Proof" surface, opened over the war room.
//
// It is a thin DOM instrument floating over the Canvas (pointer events fall through
// its empty regions to the orbit camera; only the buttons capture). It reads as a
// flight-deck switch: the dim sibling, the active view lit acid-green with a
// scanning radar-sweep underline. Styling is in style.css (.wd-nav*) so the sweep
// animation and phosphor tokens stay with the rest of the chrome.
//
// The pure view model (`VIEWS`, `navItemProps`) is unit-tested in node; the rendered
// bar (a11y `aria-current`, keyboard focus, the sweep) is verified live.

import { Fragment } from 'react';

/** The two top-level surfaces. RADAR is the war room; DOSSIER is the profile overlay. */
export type NavView = 'radar' | 'dossier';

export const PRIMARY_VIEW: NavView = 'radar';

export const VIEWS: ReadonlyArray<{ id: NavView; label: string; hint: string; glyph: string }> = [
  { id: 'radar', label: 'Radar', hint: 'live agent fleet', glyph: '◎' },
  { id: 'dossier', label: 'Dossier', hint: 'profile by proof', glyph: '❖' },
];

/** Per-item props the button spreads — `aria-current="page"` only on the active view. */
export function navItemProps(
  id: NavView,
  active: NavView,
): { active: boolean; 'aria-current'?: 'page' } {
  const isActive = id === active;
  return isActive ? { active: true, 'aria-current': 'page' } : { active: false };
}

export function NavBar({
  view,
  onView,
  counts,
}: {
  /** The surface currently on screen (RADAR = war room, DOSSIER = profile open). */
  view: NavView;
  onView: (v: NavView) => void;
  /** Live size of each surface, shown as a chip. Honest: omit/0 = nothing there. */
  counts?: Partial<Record<NavView, number>>;
}) {
  return (
    <nav className="wd-nav" aria-label="View">
      <span className="wd-nav-mark" aria-hidden>
        ✦
      </span>
      {VIEWS.map((v, i) => {
        const props = navItemProps(v.id, view);
        const n = counts?.[v.id];
        return (
          <Fragment key={v.id}>
            {i > 0 && <span className="wd-nav-div" aria-hidden />}
            <button
              type="button"
              className={`wd-nav-tab${props.active ? ' is-active' : ''}`}
              aria-current={props['aria-current']}
              title={v.hint}
              onClick={() => onView(v.id)}
            >
              <span className="wd-nav-glyph" aria-hidden>
                {v.glyph}
              </span>
              <span className="wd-nav-label">{v.label}</span>
              {typeof n === 'number' && (
                <span className={`wd-nav-count${n === 0 ? ' is-zero' : ''}`}>{n}</span>
              )}
              <span className="wd-nav-sweep" aria-hidden />
            </button>
          </Fragment>
        );
      })}
    </nav>
  );
}

export default NavBar;
