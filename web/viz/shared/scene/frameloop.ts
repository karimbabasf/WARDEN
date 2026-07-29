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
//   visible, blurred -> 'demand'  paced by BackgroundFrameTick, roughly 8fps
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

// Frames per second for the visible-but-unfocused state. Fast enough that liveness
// still reads at a glance and the scene does not look frozen, slow enough to be roughly
// an eighth of the render cost.
export const BACKGROUND_FPS = 8;
