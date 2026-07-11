// Breadcrumb.tsx: the radar focus path (Overview > agent > ...), rendered from
// WarRoom's `focusStack` of agent ids. "Overview" clears focus; each crumb pops
// focus to that depth. Renders nothing when the stack is empty (Overview is implicit
// then). Labels are looked up from the live agents so the trail reads as agent names,
// not raw ids.

import { Fragment } from 'react';

export function Breadcrumb({
  focusStack,
  agents,
  onPopFocus,
  onClearFocus,
}: {
  focusStack: string[];
  agents: readonly { id: string; label: string }[];
  onPopFocus: (index: number) => void;
  onClearFocus: () => void;
}) {
  if (focusStack.length === 0) return null;
  const labelFor = (id: string) => agents.find((a) => a.id === id)?.label ?? id;
  return (
    <nav className="wd-breadcrumb" aria-label="Focus path">
      <button type="button" className="wd-crumb wd-crumb-root" onClick={onClearFocus} title="Back to overview">
        Overview
      </button>
      {focusStack.map((id, i) => {
        const last = i === focusStack.length - 1;
        return (
          <Fragment key={`${id}-${i}`}>
            <span className="wd-crumb-sep" aria-hidden="true">&rsaquo;</span>
            <button
              type="button"
              className={`wd-crumb${last ? ' is-current' : ''}`}
              aria-current={last ? 'location' : undefined}
              onClick={() => onPopFocus(i)}
              title={`Focus ${labelFor(id)}`}
            >
              {labelFor(id)}
            </button>
          </Fragment>
        );
      })}
    </nav>
  );
}

export default Breadcrumb;
