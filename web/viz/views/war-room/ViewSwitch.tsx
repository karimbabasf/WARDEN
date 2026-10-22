// ViewSwitch.tsx: Radar or Brickyard. Two looks at the same agents: the radar reads the
// fleet at a glance, the brickyard turns each agent's tool calls into a real build.

export type WardenView = 'radar' | 'brickyard';

const VIEW_KEY = 'warden.view';

/** A workspace preference like the rail fold, so it survives a reload. */
export function readView(): WardenView {
  try {
    return window.localStorage.getItem(VIEW_KEY) === 'brickyard' ? 'brickyard' : 'radar';
  } catch {
    return 'radar';
  }
}

export function writeView(v: WardenView): void {
  try {
    window.localStorage.setItem(VIEW_KEY, v);
  } catch {
    /* no storage: the switch still works, it just does not survive a reload */
  }
}

export function ViewSwitch({ view, onView }: { view: WardenView; onView: (v: WardenView) => void }) {
  return (
    <div className="wd-view-switch" role="group" aria-label="View">
      <button type="button" className="wd-view-seg" aria-pressed={view === 'radar'} onClick={() => onView('radar')}>
        Radar
      </button>
      <button type="button" className="wd-view-seg" aria-pressed={view === 'brickyard'} onClick={() => onView('brickyard')}>
        Brickyard
      </button>
    </div>
  );
}
