// radarAlert.ts: the pure look of the THIRD globe state, "this one is waiting on you".
//
// The board already speaks two states fluently. A working globe blazes and rides a calm
// ~4.2s sine breath; an idle one sits dim and perfectly steady. Awaiting has to be
// unmistakable against BOTH of those at a glance, from across a room, in peripheral
// vision, which is the only way an alert state is worth anything.
//
// So it does not just change one channel, it changes the WAVEFORM:
//
//   working   smooth sine, slow, hue stays the harness identity, spins faster
//   idle      flat, dim, hue dulled
//   awaiting  a hard-edged DOUBLE FLASH on a short cycle, in alert red, spinning at
//             the resting rate
//
// A sine says "breathing". A pair of sharp strobes with a dark gap after them says
// "beacon", the way an aircraft anti-collision light does, and no amount of squinting
// turns one into the other. That is deliberate: brightness alone would be a busier
// working globe, and colour alone fails for a colour-blind operator. The cadence is the
// channel that survives both.
//
// Two rules that look like details and are not:
//
//   • the flash is NOT phase-scattered per globe. Working globes are (the forest should
//     breathe organically); alerts must fire in LOCKSTEP, because three agents waiting
//     on you should read as one alarm, not as a twitching board.
//   • it never goes fully dark. A globe that vanishes between flashes cannot be located,
//     and the operator has to be able to point at the thing that is asking.
//
// Pure (no Three.js, no React), so the waveform is unit-tested directly.

/**
 * Alert red. Deliberately a CRIMSON, not the orange-red of the terminated flare
 * (`#ff5a37`) and nowhere near Claude's tangerine (`#ff8636`), so a waiting globe can
 * never be misread as a Claude globe that happens to be bright, or as one imploding.
 */
export const ALERT_HEX = '#ff2740';

/** Seconds for one full flash cycle: two strobes, then a rest. */
export const ALERT_PERIOD = 1.55;

/** Where each strobe starts within the cycle (seconds from the top). */
const PULSE_STARTS = [0, 0.26];

/** How long one strobe lasts, rise and fall included. */
const PULSE_WIDTH = 0.2;

/**
 * Peakiness of a strobe. A raw half-sine is still a soft swell; raising it sharpens the
 * shoulders into an attack and a decay while keeping the curve continuous, so the flash
 * reads hard-edged without ever stepping (a step would strobe against the frame rate).
 */
const PULSE_SHARPNESS = 1.7;

/**
 * The level between flashes. Above zero on purpose: dark enough that the strobe reads as
 * a strobe, bright enough that the globe never disappears from the board.
 */
export const ALERT_FLOOR = 0.2;

/**
 * The level a reduced-motion viewer sees instead of the flash: a steady, high burn.
 *
 * `prefers-reduced-motion` asks us to drop restless motion, and a 1.55s strobe is the
 * definition of it. What it does NOT ask is to drop the information, so the globe holds
 * at a level ABOVE a working globe's peak and keeps the red and the label. The state
 * stays legible; only the flashing goes.
 */
export const ALERT_STEADY = 0.88;

function clamp01(v: number): number {
  return Number.isFinite(v) ? Math.max(0, Math.min(1, v)) : 0;
}

/**
 * The alert waveform at time `t` (seconds), 0..1.
 *
 * `reduced` (prefers-reduced-motion) returns the steady level instead, with no
 * dependence on `t` at all, so nothing animates.
 */
export function alertBlink(t: number, reduced = false): number {
  if (reduced) return ALERT_STEADY;
  if (!Number.isFinite(t)) return ALERT_FLOOR;
  const phase = ((t % ALERT_PERIOD) + ALERT_PERIOD) % ALERT_PERIOD;
  let peak = 0;
  for (const start of PULSE_STARTS) {
    const d = phase - start;
    if (d < 0 || d >= PULSE_WIDTH) continue;
    peak = Math.max(peak, Math.pow(Math.sin((Math.PI * d) / PULSE_WIDTH), PULSE_SHARPNESS));
  }
  return ALERT_FLOOR + (1 - ALERT_FLOOR) * clamp01(peak);
}

/**
 * The emissive/halo multiplier an awaiting globe applies on top of its glow target.
 *
 * The trough sits just under a resting globe and the crest well over a working one, so
 * the flash sweeps THROUGH the whole board's brightness range every cycle. That sweep is
 * what catches the eye in peripheral vision, where absolute brightness does not.
 */
export function alertGlowMultiplier(blink: number): number {
  return 0.55 + clamp01(blink) * 2.35;
}

/**
 * How far the globe's colour is pushed toward white at the top of a flash.
 *
 * Small on purpose. The red IS the signal, and washing it out at the crest would make
 * the peak of every flash look like the white-hot core of a working globe. Kept low
 * because the bloom ALREADY saturates the core toward white at these emissive levels:
 * anything more here and the material stops carrying any red at the peak, which is the
 * one frame of the cycle the eye is most likely to catch.
 */
export function alertWhiteMix(blink: number): number {
  return clamp01(blink) * 0.14;
}
