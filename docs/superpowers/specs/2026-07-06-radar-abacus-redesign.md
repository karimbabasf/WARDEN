# RADAR Abacus Redesign: design spec

Date: 2026-07-06
Status: approved (design), pending implementation plan
Scope: frontend only (`web/viz/**`), radar tab only. No Rust, no backend, no Habits redesign.

## 1. Motivation

Today the radar is an orbitable 3D scatter. Globes float in a folder-clustered cloud
(`radarLayout.ts` lays folders side-by-side as rings) and the user rotates around it with
drei `OrbitControls`, can pan (right-drag), can enter a free-fly mode (press `F`), and can drag
individual nodes anywhere (`useNodeDrag` + `PositionOverrides`). The result is cinematic but
you *hunt*: globes occlude each other, the scene collapses edge-on when orbited, and finding a
specific agent means manual camera work. The stated goal is the opposite: spend time reading what
agents are doing, not organizing globes or looking for them. Everything should be visible and
understandable at rest.

The redesign flattens the orbit cloud into a fixed, legible board (an "abacus"): horizontal rails,
one per folder, agents as beads, subagents branching off. The camera stops being a 6DOF toy and
becomes a locked board you zoom into.

This is an evolution, not a rewrite. Three capabilities already exist and are reused:
zoom-to-cursor is already on (`CameraRig.tsx`, `zoomToCursor`), the layout is already
folder-clustered left to right (`radarLayout.ts`), and the camera already knows how to auto-fit the
whole scene and glide-focus a subtree (`cameraFraming.ts`: `enclosingBounds`, `subtreeBounds`,
`frameDistance`).

## 2. Goals and non-goals

Goals:
- Replace the 3D orbit scatter with a fixed "abacus" board: rails = folders, beads = agents,
  subagents branch downward.
- Lock the camera: no rotate, no pan, no free-fly, no node-drag. The only free camera verb is
  zoom-to-cursor. Navigation beyond that is click-to-focus, with a fit/reset back to the overview.
- Everything auto-frames to fit on load and on data change, so "all agents visible" is the resting
  state.
- Remove the Ledger and the hamburger (`☰`) roster menu from the radar. Center the filter at the
  bottom.
- Keep the design on-brand and "stunning" through sphere shading, glow, and the starfield, not
  through camera motion.

Non-goals (out of scope for this spec):
- Habits tab visuals or its camera behavior (it shares the rig; we make the lock radar-only).
- Relocating Forge write-history (the Ledger is Forge data; removing it from the radar is in scope,
  giving it a new home is a separate task).
- Any Rust, ingest, brain, or radar-signal changes. The data contract (`RadarAgent`) is unchanged.
- M5 to M7 features.

## 3. Component decisions (the five on-screen pieces)

Reference: all five mount from `web/viz/views/war-room/WarRoom.tsx` into one warm `<Canvas>`.

1. Globes / 3D agent-forest scene (`RadarForest` in `web/viz/modules/radar/RadarConstellation.tsx`):
   KEEP the scene, its `RadarGlobe`, `RadarLinks` (tethers), `RadarClusterLabels`, and
   `RadarHoverLayer`. CHANGE the node positions it consumes (new layout, section 4) and the camera
   that frames it (section 5).
2. Ledger (bottom-right, in `web/viz/views/war-room/chrome.tsx`, mounted via `<Chrome>` at
   `WarRoom.tsx`): HIDE on the radar tab. It is Forge write-history, not radar data, and currently
   renders on both tabs unconditionally. Gate its toggle button, the `Ledger` aside, and its dock on
   the radar tab (a `showLedger` flag = `displayTab !== 'radar'`), so the radar is clean while other
   tabs are unchanged. Props (`ledgerOpen`, `onToggleLedger`, `artifacts`, `reverting`, `onRevertFix`)
   and `.wd-ledger*` styles stay. Giving the Forge history a permanent home is a separate task
   (section 2, non-goals).
3. Filter (bottom, `web/viz/views/war-room/FilterBar.tsx`): KEEP, MOVE to dead-center bottom (CSS
   `.wd-filterbar` in `web/style.css`). Its radar behavior (harness color emphasis) is unchanged.
4. RADAR | HABITS switcher (`web/viz/views/war-room/NavBar.tsx`): KEEP unchanged.
5. Hamburger `☰` (inline in `WarRoom.tsx`) plus the roster `Sidebar`
   (`web/viz/views/war-room/Sidebar.tsx`, fed by `buildRadarRoster` in
   `web/viz/modules/radar/rosterTree.ts`): REMOVE the button, its `sidebarOpen` state and toggle,
   and the `Sidebar` mount. Selection moves entirely onto the beads (click a globe). `buildRadarRoster`
   / `rosterTree.ts` become dead for radar; delete if unused elsewhere.

## 4. Layout: the rail board

New layout produced by a rail layout function (either a rewrite of `layoutRadarScene` in
`web/viz/modules/radar/radarLayout.ts` or a sibling `layoutRadarBoard` selected at the two call
sites: the render path in `RadarConstellation.tsx` and the camera-framing path in `WarRoom.tsx`).
Pure and deterministic, same `RadarLayout` output shape (nodes with `[x,y,z]` + links), so the
scene and framing code downstream do not change.

Model:
- One rail per folder (`folderKey` / cwd), same grouping the current layout already computes.
- Rails stack vertically. Each rail has a folder tag at its left head and a glowing horizontal rod.
- Root agents (depth 0, or any agent whose parent does not resolve) are beads sitting on their
  folder's rod, placed left to right.
- Subagents branch downward from their parent bead as a tether-tree: a child at depth `d` sits at
  `y = railY - d * ROW_STEP` (below the rail), siblings spread horizontally and centered under the
  parent. Tethers are the existing curved parent-to-child links rendered by `RadarLinks`.
- All nodes are coplanar at `z = 0` (the board plane). Depth in the picture comes from sphere
  shading, not from Z placement, so no rail is nearer or farther than another (no parallax between
  rails).

Spatial stability (the core UX rule):
- Rails and beads hold their positions. Activity is shown by glow and size, never by reordering.
  A folder becoming active brightens; it does not jump position. This is what builds muscle memory
  and removes hunting.
- Rail order is a stable key (first-seen order of the folder in the data). Bead order within a rail
  is stable by agent start order. New agents append gently; ended agents fade per the existing
  lifecycle. No live reshuffle.

Spacing and sizing:
- Horizontal bead spacing on a rail is `max(MIN_GAP, subtreeWidth)` so a root's downward subtree
  does not collide with the next bead.
- Vertical rail gap adapts: `railGap = BASE_GAP + maxSubtreeDepth(previousRail) * ROW_STEP`, so a
  deep subtree never overlaps the rail below it.
- Node radius is unchanged: `radarRadius(contextTokens, depth)` (square-root scaled context
  occupancy plus a depth boost). Glow/brightness is unchanged (`radarGlowTarget`, liveness-driven).
  Color is harness (Claude emerald, Codex violet). Two harnesses may share a folder's rail.

Honest-viz preserved: position still maps to real folder (cwd) plus hierarchy (parentId/depth),
size to real `contextTokens`, glow to real liveness/status. No fabricated signals.

## 5. Camera and interaction: locked board

Camera is locked straight-on to the board plane (positioned on the +Z axis looking at board center,
up = +Y). Globes still render as lit 3D spheres, so the board keeps depth without disorientation.

Controls:
- Rotate: OFF. Pan: OFF. Free-fly (`FlyControls`, toggled by `F` in `WarRoom.tsx`): REMOVED,
  including the key handler and `flyMode` state. Node-drag (`useNodeDrag`, `PositionOverrides`,
  the `onMoveNode`/`overrides` wiring and its reset-on-run): REMOVED from the radar path.
- Zoom: KEEP. Scroll dollies toward the cursor (existing `zoomToCursor`).
- Implementation may keep drei `OrbitControls` with `enableRotate={false}` and `enablePan={false}`
  and polar angle pinned to the straight-on pose, or replace it with a minimal zoom-only controller.
  The plan decides; the behavior is what is fixed here.

Navigation (reuses existing framing helpers in `CameraRig.tsx` and `cameraFraming.ts`):
- Click a bead: select it (existing `selectedId`), open the detail panel (`RadarDetailPanel`), and
  glide-frame it (existing select-to-focus glide).
- Click a folder tag: frame that whole rail (`enclosingBounds` / `subtreeBounds` of that folder's
  nodes, via the existing 700ms `focusBounds` fly).
- Esc, click on empty space, or a small fit-chip: ease back to the auto-framed overview (existing
  `homeSignal`).
- On data change: auto-fit reframes so every rail stays visible (existing `sceneBounds` recompute).

Tab-awareness: the camera rig is shared with Habits (mounted once in `SceneShell`). The lock and the
straight-on pose apply only when `displayTab === 'radar'`. Habits keeps its current camera. This
means threading a mode (or `locked`) prop into `CameraRig`.

## 6. Scale and overflow (edge cases, stated not hidden)

- Many folders or a very wide rail: fit zooms out to hold everything. Bead and label glyphs get a
  minimum screen-size floor so the zoomed-out overview stays readable rather than collapsing to dots.
- If the number of folders exceeds the vertical budget even at min glyph size, the board pages
  vertically: a bounded 1D scroll through the rails, not a free 2D pan. Click-to-focus and zoom
  remain the primary way in.
- A deep or wide subtree just extends the tree; the adaptive `railGap` (section 4) reserves room and
  fit accounts for it.
- Empty state (no agents): a calm empty board with a short hint, no rails.

## 7. States

- Active agent: full size, bright glow, gentle pulse.
- Idle agent: dimmed, slightly smaller, no pulse.
- Terminated/ended agent: fades and is removed per the existing lifecycle (parent tool-result or the
  90s backstop). No resurrect.
- Selected: emphasized ring plus the detail panel; camera framed on it.
- Filtered (harness emphasis from the centered FilterBar): non-matching beads dim (color-only), as
  today.

## 8. Files touched (anchors, may drift during implementation)

Change:
- `web/viz/modules/radar/radarLayout.ts` (rail layout function; the two call sites in
  `RadarConstellation.tsx` and `WarRoom.tsx`).
- `web/viz/shared/scene/CameraRig.tsx` (lock rotate/pan, straight-on pose, tab-aware `locked` prop;
  keep zoom-to-cursor and the framing helpers).
- `web/viz/views/war-room/WarRoom.tsx` (remove `☰` button, `sidebarOpen`, `Sidebar` mount, `F`/fly
  handler and `flyMode`, node-drag `overrides`/`onMoveNode`; thread `locked` to `CameraRig`; pass
  `showLedger = displayTab !== 'radar'` to `<Chrome>`).
- `web/viz/views/war-room/chrome.tsx` (gate the Ledger toggle, aside, and dock on a `showLedger`
  prop; no code deleted).
- `web/viz/views/war-room/FilterBar.tsx` + `web/style.css` (center the filter; remove `.wd-side-toggle`
  and sidebar styles; keep `.wd-ledger*`, radar-hidden not deleted).

Remove (delete if unused after the above):
- `web/viz/shared/scene/useNodeDrag.ts` and `web/viz/shared/scene/positionOverrides.ts` (radar was
  the only consumer; confirm before deleting).
- `FlyControls` usage in `CameraRig.tsx`.
- `web/viz/views/war-room/Sidebar.tsx` and `web/viz/modules/radar/rosterTree.ts`
  (`buildRadarRoster`) if unused elsewhere.

Keep, unchanged in behavior:
- `RadarConstellation.tsx` scene internals (`RadarGlobe`, `RadarLinks`, `RadarClusterLabels`,
  `RadarHoverLayer`), `RadarDetailPanel`, `NavBar.tsx`, `radarTheme.ts`, the `RadarAgent` type.

## 9. Prototyping approach

Iterate the rail layout and locked camera in `radar-lab.html` (served from
`web/viz/dev/preview/radarLab.tsx`, which mounts the real `RadarConstellation` + `RadarDetailPanel`
with a hardcoded mock forest, no Tauri backend). Validate layout, stability, subagent branching, and
the zoom/click-to-focus/fit interactions there against both the mock forest and the captured
`web/viz/dev/preview/realRadar.json` before wiring into `WarRoom.tsx`. Verify in the browser via the
preview tools.

## 10. Acceptance criteria (checkable)

- Radar has no rotate, no pan, no free-fly (`F` does nothing), and no node-drag (dragging a globe
  does nothing).
- Scroll zooms toward the cursor.
- Click a bead frames it and opens the detail panel; click a folder tag frames that rail; Esc, an
  empty-space click, or the fit-chip returns to the auto-framed overview.
- The board renders one rail per folder, beads stable in position, subagents branching downward with
  tethers; activity is shown by glow/size, never by reordering.
- On load and on data change, all rails are visible (auto-fit).
- Ledger and `☰` are gone from the radar; the filter sits dead-center at the bottom.
- Habits tab camera is unaffected.
- `pnpm build` is clean, `pnpm check:arch` passes, and radar-related tests are updated and green.
- The prototype in `radar-lab.html` demonstrates the above before the change lands in the live app.
