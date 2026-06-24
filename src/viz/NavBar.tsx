// NavBar.tsx — the top constellation switch (Habits | Radar).
//
// One overlay, two scenes: the war-room renders either the persistent anti-pattern
// "Habits" constellation or the live "Radar" agent forest, chosen by a tab. This
// bar is a thin DOM control floating over the Canvas (pointer events fall through
// its empty regions to the orbit camera; only the buttons capture). Styling uses
// the app phosphor tokens from style.css so it reads as part of the chrome.
//
// The pure tab model (`TABS`, `navItemProps`) is unit-tested in node; the rendered
// bar (a11y `aria-current`, keyboard focus) is verified live in the dev harness.

import type { CSSProperties } from 'react';

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

const barStyle: CSSProperties = {
  position: 'fixed',
  top: 18,
  left: '50%',
  transform: 'translateX(-50%)',
  zIndex: 7,
  display: 'flex',
  gap: 4,
  padding: 4,
  borderRadius: 999,
  background: 'var(--panel, rgba(4,18,11,0.66))',
  border: '1px solid var(--hair, rgba(118,255,157,0.20))',
  backdropFilter: 'blur(8px)',
  WebkitBackdropFilter: 'blur(8px)',
  fontFamily: 'var(--mono, monospace)',
};

function tabStyle(active: boolean): CSSProperties {
  return {
    appearance: 'none',
    cursor: 'pointer',
    border: 'none',
    borderRadius: 999,
    padding: '7px 18px',
    fontFamily: 'inherit',
    fontSize: 11,
    letterSpacing: '0.22em',
    textTransform: 'uppercase',
    transition: 'color 160ms ease, background 160ms ease, box-shadow 160ms ease',
    color: active ? 'var(--bg, #020403)' : 'var(--ink-faint, #5f8a6f)',
    background: active ? 'var(--green, #76ff9d)' : 'transparent',
    boxShadow: active ? '0 0 18px rgba(118,255,157,0.45)' : 'none',
    fontWeight: active ? 600 : 500,
  };
}

export function NavBar({ tab, onTab }: { tab: ConstellationTab; onTab: (t: ConstellationTab) => void }) {
  return (
    <nav className="wd-nav" style={barStyle} aria-label="Constellation">
      {TABS.map((t) => {
        const props = navItemProps(t.id, tab);
        return (
          <button
            key={t.id}
            type="button"
            className={`wd-nav-tab${props.active ? ' is-active' : ''}`}
            style={tabStyle(props.active)}
            aria-current={props['aria-current']}
            title={t.hint}
            onClick={() => onTab(t.id)}
          >
            {t.label}
          </button>
        );
      })}
    </nav>
  );
}

export default NavBar;
