// ConstellationSwitcher.tsx: which board am I looking at, yours or theirs.
//
// Watching a peer puts a SECOND constellation in the world, parked beside your own
// (peerFraming) and marked as not-yours (PeerConstellation's hazard curtain). Both
// stay mounted, and the camera trucks laterally between them, so the two boards and
// the distance between them stay readable the whole way over.
//
// What was missing was the control. The camera moved to the peer the instant a frame
// arrived and there was no way back except stopping watching altogether, which meant
// "watch someone" silently cost you your own board. This is that way back: a two-way
// switch at the top of the free channel, one segment per constellation, so the truck
// is something you ask for in both directions.
//
// It renders NOTHING when there is no peer board. A switcher with one segment is not
// a switcher, and the local board is the only thing to look at until a peer sends a
// frame (a selected-but-silent peer has no constellation, see WarRoom's peerHasAgents).

export type ConstellationTarget = 'local' | 'peer';

export type ConstellationSwitcherProps = {
  /** Live agents on your own board. */
  localCount: number;
  /** The watched peer's label, already redacted upstream. Null = nobody watched. */
  peerLabel: string | null;
  /** Agents on the peer's board. Zero means they have sent no frame worth showing. */
  peerCount: number;
  target: ConstellationTarget;
  onTarget: (target: ConstellationTarget) => void;
};

function count(n: number): string {
  return `${n} agent${n === 1 ? '' : 's'}`;
}

export function ConstellationSwitcher({
  localCount,
  peerLabel,
  peerCount,
  target,
  onTarget,
}: ConstellationSwitcherProps) {
  if (!peerLabel || peerCount === 0) return null;

  return (
    // A radiogroup, not a tablist: nothing is hidden or revealed, the same scene is
    // framed two ways. Arrow keys move between the two segments for free.
    <div className="wd-constellation-switch" role="radiogroup" aria-label="Which constellation to view">
      <button
        type="button"
        role="radio"
        aria-checked={target === 'local'}
        className={`wd-constellation-seg${target === 'local' ? ' is-active' : ''}`}
        onClick={() => onTarget('local')}
      >
        <span className="wd-constellation-seg-name">Your agents</span>
        <span className="wd-constellation-seg-count">{count(localCount)}</span>
      </button>
      <button
        type="button"
        role="radio"
        aria-checked={target === 'peer'}
        className={`wd-constellation-seg is-peer${target === 'peer' ? ' is-active' : ''}`}
        onClick={() => onTarget('peer')}
      >
        {/* The amber dot is the same "this came off another machine" mark the peer's
            3D curtain and the observe panel wear, so the three readings agree. */}
        <span className="wd-constellation-seg-mark" aria-hidden />
        <span className="wd-constellation-seg-name">{peerLabel}</span>
        <span className="wd-constellation-seg-count">{count(peerCount)}</span>
      </button>
    </div>
  );
}

export default ConstellationSwitcher;
