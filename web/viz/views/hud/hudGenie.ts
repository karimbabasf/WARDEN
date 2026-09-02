// hudGenie.ts: the HUD panel funnelling back up into the tray icon.
//
// macOS's own genie is a private window-server effect that only minimises to the
// Dock, so this rebuilds it honestly out of one animated `clip-path: polygon()`.
// The polygon is what makes it a genie rather than a scale-down: the panel's left
// and right edges are sampled at ~20 heights and each sample is pulled toward the
// icon's own edges on its OWN schedule, so the top of the panel is already a narrow
// neck while the bottom is still full width. That funnel shape is the whole effect.
//
// `clip-path` clips the element AND its whole subtree, which is why the HUD draws its
// globes into a canvas that lives inside the panel: the WebGL globes and the DOM
// labels bend through the same neck, in one pass, with no second implementation.
//
// Direction: t = 0 is fully open, t = 1 is fully swallowed. Opening does NOT run this
// (see HudPanel): the panel springs open like a Dynamic Island and genies shut, which
// is the asymmetry the user asked for and also the right one: a genie played backwards
// at 300ms reads as a smear, while the expansion has to read as an arrival.
//
// Pure module: no React, no DOM. Unit-tested in hudGenie.test.ts.

export type GenieGeometry = {
  /** Panel size in CSS px. */
  width: number;
  height: number;
  /** The tray icon's edges, in PANEL-LOCAL px. May sit outside [0, width] when the
   *  panel had to be pushed off the icon to stay on screen; the funnel then leans,
   *  which is correct, it still points at the button that owns it. */
  neckLeft: number;
  neckRight: number;
};

export type GenieFrame = {
  clipPath: string;
  /** Applied to the panel's CONTENT, not the panel: the pixels slide up through the
   *  neck while the clip narrows around them. */
  contentTransform: string;
  contentOrigin: string;
  contentOpacity: number;
};

/** How much later the BOTTOM of the panel starts collapsing than the top. 0 would
 *  narrow every row together (a squeeze, not a genie); too high and the bottom edge
 *  is still full-width when the animation ends. */
const SPREAD = 0.62;

/** Vertical samples down each edge. Twenty is where the neck stops looking faceted;
 *  more just costs string length on every frame. */
const SAMPLES = 20;

function clamp01(v: number): number {
  return v < 0 ? 0 : v > 1 ? 1 : v;
}

/** Smootherstep: zero first AND second derivative at both ends, so the funnel has no
 *  visible kink where a row starts or finishes moving. */
export function smootherstep(v: number): number {
  const x = clamp01(v);
  return x * x * x * (x * (x * 6 - 15) + 10);
}

function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

function round(v: number): number {
  return Math.round(v * 100) / 100;
}

/**
 * The clip polygon and content transform for collapse progress `t`.
 *
 * At t = 0 this is exactly the panel's rectangle, so the same code path can drive a
 * fully open panel without a special case (and without a `clip-path: none` swap that
 * would drop the element out of its own compositing layer mid-animation).
 */
export function genieFrame(t: number, geo: GenieGeometry): GenieFrame {
  const p = clamp01(t);
  const { width, height, neckLeft, neckRight } = geo;
  const pts: string[] = [];
  const right: string[] = [];
  const left: string[] = [];

  for (let i = 0; i <= SAMPLES; i++) {
    const v = i / SAMPLES; // 0 at the panel's top edge, 1 at its bottom
    // Rows nearer the neck start first: at p = 0 every phase is 0, at p = 1 every
    // phase is 1, and in between the schedule sweeps top to bottom.
    const phase = smootherstep(p * (1 + SPREAD) - v * SPREAD);
    const y = round(v * height * (1 - p));
    right.push(`${round(lerp(width, neckRight, phase))}px ${y}px`);
    left.push(`${round(lerp(0, neckLeft, phase))}px ${y}px`);
  }
  // Down the right edge, then back up the left one.
  pts.push(...right);
  for (let i = left.length - 1; i >= 0; i--) pts.push(left[i]);

  const neckCentre = (neckLeft + neckRight) / 2;
  return {
    clipPath: `polygon(${pts.join(', ')})`,
    // Non-uniform on purpose, and this is the half that makes it a genie rather than a
    // clipped rectangle: the horizontal squeeze is ~3.5x the vertical one, so the
    // CONTENT converges on the neck at the same rate the clip does. With a uniform
    // scale the pixels stay a grid while the silhouette funnels around them, and the
    // eye reads two effects. Origin is the neck, so both axes pull toward the icon.
    contentTransform:
      `translate3d(0, ${round(-p * height * 0.2)}px, 0) ` +
      `scale(${round(1 - p * 0.72)}, ${round(1 - p * 0.2)})`,
    contentOrigin: `${round(neckCentre)}px 0px`,
    // Held opaque until the shape has already done the reading; a fade that starts
    // at t = 0 makes the whole thing look like a plain dissolve.
    contentOpacity: round(1 - smootherstep((p - 0.62) / 0.38)),
  };
}

/** How long the collapse runs. Around 300ms: the close is the system responding, and
 *  the user has already decided. It is eased (below), so it reads a touch quicker
 *  than a linear 300 would. */
export const GENIE_MS = 300;

/**
 * Elapsed time to collapse progress.
 *
 * Linear time was the one mechanical thing left in the close: the bottom edge swept
 * up at one constant speed and stopped dead at the neck. Smoothstep leaves from rest,
 * gathers speed through the middle where the funnel does its reading, and lands in
 * the icon instead of hitting it. Zero velocity at both ends, so it also joins the
 * still panel before it and the hidden window after it without a kink.
 */
export function genieProgress(elapsedMs: number): number {
  const t = clamp01(elapsedMs / GENIE_MS);
  return t * t * (3 - 2 * t);
}
