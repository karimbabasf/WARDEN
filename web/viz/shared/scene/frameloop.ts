// Maps window state to React-Three-Fiber's <Canvas frameloop> mode. Shared by every
// scene root (the war-room shell and the radar forest) so the rule lives in one place
// instead of being read upward from a view.
//
// THREE states, not two. WARDEN is a monitor: it is meant to sit open all day next to
// the work it watches, which means "visible but not focused" is its NORMAL state, not a
// rare one. Rendering a rotating constellation at display rate in that state cost about
// half a core between the WebKit GPU process and WindowServer, for frames nobody was
// looking at. `document.hidden` does not catch it either: on macOS that flag only goes
// true when the window is minimized or the app is hidden, never when it is merely behind
// another window.
//
//   hidden/minimized -> 'never'   the loop stops entirely
//   visible, blurred -> 'demand'  paced by BackgroundFrameTick, at BACKGROUND_FPS
//   focused          -> 'always'  full display rate, unchanged
//
// 'demand' renders nothing on its own, so a Canvas using it MUST mount
// <BackgroundFrameTick paced /> or the scene freezes. Every animation in the scene is
// driven by useFrame's `dt` (exponential damping, lerps), so a lower rate stays visually
// correct, just coarser.
export type FrameloopMode = 'always' | 'demand' | 'never';

export function frameloopFor(hidden: boolean, focused = true): FrameloopMode {
  if (hidden) return 'never';
  return focused ? 'always' : 'demand';
}

// Frames per second for the visible-but-unfocused state.
//
// This was 8, picked purely as a cost number, and that was the wrong end of the
// trade: blurred is the state WARDEN spends its life in, so it is also the state the
// user spends the most time LOOKING at, and a permanently-rotating constellation at
// 8fps reads as broken rather than as economical. 30 is the floor where continuous
// motion still reads as motion; below it the eye starts resolving single frames.
//
// The cost argument survives intact. What made blurred rendering expensive was that
// it ran at DISPLAY rate, which on a 120Hz panel is 120fps; 30 is a quarter of that.
// And the pacer now rides rAF (see BackgroundFrameTick), so a fully occluded or
// off-screen window drops to zero frames instead of the steady setInterval drip it
// used to pay: the cheap case got cheaper as the visible case got smooth.
export const BACKGROUND_FPS = 30;

// A paced tick fires once this fraction of the nominal period has elapsed.
//
// The pacer rides rAF, so it can only fire on a display refresh. At 30fps on a 60Hz
// panel the period (33.3ms) is exactly two refreshes, and a strict `>=` comparison
// loses that race to floating-point jitter about half the time, slipping the tick to
// the third refresh so the rate alternates 30/20/30/20. That uneven spacing reads as
// judder even though the average rate looks right. Accepting a tick that is
// marginally early snaps the cadence onto the nearest refresh and holds it there.
const TICK_TOLERANCE = 0.85;

/**
 * Should the background pacer render a frame now? `sinceLastMs` is the time since the
 * last paced frame. Pure, so the cadence is unit-tested without a browser.
 *
 * A non-finite input (the first tick, a clock that jumped) always fires: a dropped
 * frame is a stall, and every animation downstream is `dt`-driven, so an early frame
 * is harmless.
 */
export function backgroundTickDue(sinceLastMs: number, fps: number = BACKGROUND_FPS): boolean {
  if (!Number.isFinite(sinceLastMs)) return true;
  const period = 1000 / Math.max(1, fps);
  return sinceLastMs >= period * TICK_TOLERANCE;
}
