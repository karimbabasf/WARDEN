// Maps a hidden flag to React-Three-Fiber's <Canvas frameloop> mode: a hidden
// canvas stops its render loop (CPU saver), a visible one renders continuously.
// Shared by every scene root (the war-room shell and the radar forest) so the
// rule lives in one place instead of being read upward from a view.
export function frameloopFor(hidden: boolean): 'always' | 'never' {
  return hidden ? 'never' : 'always';
}
