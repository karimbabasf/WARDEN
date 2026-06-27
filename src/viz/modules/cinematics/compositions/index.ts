// compositions/index.ts — the Remotion composition barrel (spec §6.5).
//
// Re-exports the three composition roles + the pure timing helpers. This module
// pulls in the .tsx components (and therefore `remotion`), so it is the LAZY
// boundary: nothing in the summon hot path imports it directly — WarRoom.tsx
// `React.lazy()`-loads the player host, which is the only thing that reaches
// these. Keeping the import graph here means Remotion lands in its own Vite
// chunk, never in the main bundle (risk R-Bundle).

export { FPS, INTRO_FRAMES, revealDuration } from './timing';
export { Intro, INTRO_DURATION } from './Intro';
export { Reveal, type RevealFinding, type RevealProps } from './Reveal';
export { Recap, recapDuration, type RecapProps } from './Recap';
