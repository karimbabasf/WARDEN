// awaitingCopy.ts: the words for the third globe state, in one place.
//
// Colour is never the only signal in WARDEN, so the red strobe always ships with a word.
// That word appears on the strip, the hover card and the detail panel, and those three
// live in different layers, so the copy lives HERE (shared) rather than being defined in
// the rail and reached for sideways.
//
// Written for the operator, not the harness: "Asked you a question" rather than
// "AskUserQuestion pending". The reason vocabulary is closed (mirrors Rust
// `AwaitingReason`), so this map is total by construction.

import type { RadarAwaitingReason } from '@/viz/shared/types/radarTypes';

/** One line for a strip or a panel row: what this agent wants from you. */
export const AWAITING_LINE: Record<RadarAwaitingReason, string> = {
  question: 'Asked you a question',
  approval: 'Waiting for your approval',
  input: 'Waiting for your input',
};

/** Two or three words, for a hover card's status slot next to "Working" / "Idle". */
export const AWAITING_SHORT: Record<RadarAwaitingReason, string> = {
  question: 'Asked a question',
  approval: 'Needs approval',
  input: 'Needs input',
};

/** The line for an agent whose reason did not survive (or predates the field). */
export function awaitingLine(reason: RadarAwaitingReason | null | undefined): string {
  return AWAITING_LINE[reason ?? 'input'];
}

/** The short form, same fallback. */
export function awaitingShort(reason: RadarAwaitingReason | null | undefined): string {
  return AWAITING_SHORT[reason ?? 'input'];
}
