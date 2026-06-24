// NavBar.tsx — the top constellation switch (Habits | Radar).
//
// One overlay, two scenes: the war-room renders either the persistent anti-pattern
// "Habits" constellation or the live "Radar" agent forest, chosen by a tab. This
// bar is a thin DOM control floating over the Canvas (pointer events fall through
// its empty regions to the orbit camera; only the buttons capture). It reads as a
// flight-deck instrument: dim siblings, the active view lit acid-green with a
// scanning radar-sweep underline, and a live count chip per view (real findings /
// real live agents — never a fabricated number). Styling is in style.css (.wd-nav*)
// so the sweep animation and phosphor tokens stay with the rest of the chrome.
//
// The pure tab model (`TABS`, `navItemProps`) is unit-tested in node; the rendered
// bar (a11y `aria-current`, keyboard focus, the sweep) is verified live.

import { Fragment } from 'react';

export type ConstellationTab = 'habits' | 'radar';

export const TABS: ReadonlyArray<{ id: ConstellationTab; label: string; hint: string }> = [
  { id: 'habits', label: 'Habits', hint: 'anti-pattern mind-map' },
  { id: 'radar', label: 'Radar', hint: 'live agent forest' },
];

/** Per-tab props the button spreads — `aria-current="page"` only on the active tab. */
export function navItemProps(
  id: ConstellationTab,
  active: ConstellationTab,
): { active: boolean; 'aria-current'?: 'page' } {
  const isActive = id === active;
  return isActive ? { active: true, 'aria-current': 'page' } : { active: false };
}

export function NavBar({
  tab,
  onTab,
  counts,
}: {
  tab: ConstellationTab;
  onTab: (t: ConstellationTab) => void;
  /** Live size of each constellation, shown as a chip. Honest: omit/0 = nothing there. */
  counts?: Partial<Record<ConstellationTab, number>>;
}) {
  return (
    <nav className="wd-nav" aria-label="Constellation">
      <span className="wd-nav-mark" aria-hidden>
        ✦
      </span>
      {TABS.map((t, i) => {
        const props = navItemProps(t.id, tab);
        const n = counts?.[t.id];
        return (
          <Fragment key={t.id}>
            {i > 0 && <span className="wd-nav-div" aria-hidden />}
            <button
              type="button"
              className={`wd-nav-tab${props.active ? ' is-active' : ''}`}
              aria-current={props['aria-current']}
              title={t.hint}
              onClick={() => onTab(t.id)}
            >
              <span className="wd-nav-label">{t.label}</span>
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
