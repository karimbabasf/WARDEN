# Discovery Mode Double-Click Home Design

## Goal

While browsing the WARDEN constellation in Discovery Mode, a double-click anywhere in the interface returns the camera to the normal centered overview perspective at the fully zoomed-out distance.

## Scope

Discovery Mode means no globe is selected and no radar drill-in focus is active. If a globe is selected, a detail panel is open because of that selection, or the radar focus stack is non-empty, the double-click gesture does nothing.

The gesture should work across both Habits and Radar views while the user is freely orbiting or scroll-zooming the scene. It should not dismiss overlays, switch tabs, clear legend filters, start or stop diagnosis runs, or alter backend state.

## Interaction

The root `WarRoom` interface handles double-click. Interactive form controls and buttons keep their normal behavior; their double-clicks are not interpreted as camera home commands.

On a valid Discovery Mode double-click:

- clear transient hover state;
- keep selection state unchanged because it must already be empty;
- keep filters, tab, live scene data, and run state unchanged;
- send a one-shot reset signal to the shared camera rig.

## Camera Behavior

`CameraRig` receives a monotonically increasing reset token. When the token changes, it animates to the canonical home pose:

- orbit target: `(0, 0, 0)`;
- camera position: `(0, 1, OVERVIEW_DIST)`;
- zoom/FOV settles back through the existing FOV taper until the overview lens is restored.

This explicit home reset is different from the existing focus-backout behavior. Backing out from a selected globe still restores the user's previous browsing angle. Double-click home only runs while already browsing and always returns to the normal centered overview.

## Testing

Add pure helper tests for the Discovery Mode guard so jsdom does not need WebGL:

- double-click is allowed when no selection and no focus stack are active;
- double-click is blocked when a globe is selected;
- double-click is blocked when radar focus is active;
- double-click is blocked for interactive controls such as buttons and inputs.

Add a camera helper test for the canonical home pose so the reset target remains stable.

## Verification

Run the focused frontend tests, then the frontend build:

- `pnpm vitest run src/viz/WarRoom.test.ts src/viz/useOrbCamera.test.ts`
- `pnpm build`
