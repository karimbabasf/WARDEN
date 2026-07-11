# Hand mode (archived): radar node dragging

This branch parks the "hand mode" experiment so it can be picked up again later. It is
NOT part of the shipped radar. The live board uses a locked, straight-on camera with no
node dragging. This folder sits on top of the current radar codebase so the feature can
be rebuilt against it.

## What it was

A pointer-driven grab-and-drag of radar globes. Pressing on a globe switched the cursor
to a hand, disabled the orbit controls, and let you drag the globe on a camera-facing
plane. Releasing committed the new position as a per-agent override that survived live
data updates, so a manually-arranged board stayed put. A double-click on empty space
reset every override back to the honest computed layout.

## The files

- `useNodeDrag.ts`: the drag hook. `useNodeDrag(onCommit)` returns `{ begin, dragRef,
  movedRef, draggingId }`. `begin(id, startWorld)` disables OrbitControls and raycasts
  onto a camera-facing plane; pointermove writes the live position to `dragRef`;
  pointerup commits `[x, y, z]` through `onCommit` and re-enables the controls.
- `positionOverrides.ts`: the sticky-position map. `PositionOverrides` is a
  `Map<id, [x, y, z]>`, with `NO_OVERRIDES` and `applyLayoutOverrides(layout, overrides)`.

## How it was wired

- The globe mesh called `drag.begin(id, worldPos)` on `onPointerDown`, and its `onClick`
  was guarded by `drag.movedRef` so a drag did not register as a select.
- The view held `const [overrides, setOverrides] = useState(NO_OVERRIDES)` and reapplied
  them with `applyLayoutOverrides(layout, overrides)` after every layout pass, so dragged
  positions survived live updates. The double-click home gesture reset them to
  `NO_OVERRIDES`.

To rebuild it: drop these two files back under `web/viz/shared/scene/`, wire
`useNodeDrag` into `RadarConstellation`'s globe `onPointerDown` / `onClick`, and thread a
`PositionOverrides` state through the layout in `WarRoom`. Note that the current radar
locks the camera (`CameraRig locked`), so dragging would also need the rig unlocked (or a
drag-only exception) while a node is grabbed.
