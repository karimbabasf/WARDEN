# RADAR Abacus Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the radar's orbitable 3D scatter with a fixed "abacus" board (one horizontal rail per folder, agents as beads, subagents branching downward), lock the camera to zoom-plus-click-to-focus, and clean the chrome (remove the ledger and the roster menu from the radar).

**Architecture:** The radar's node positions come from one pure function, `layoutRadarScene(model): RadarLayout` in `web/viz/modules/radar/radarLayout.ts`. We rewrite its placement section (keeping its topology prologue and signature) to produce rails, so the two call sites and the scene renderer are unchanged. We lock the shared `CameraRig` behind a tab-aware `locked` prop, delete the radar-only drag hook, and gate the ledger + roster in `WarRoom.tsx`/`chrome.tsx`. Layout is TDD (pure geometry, vitest). Camera and chrome changes verify live in the mock harness `radar-lab.html`.

**Tech Stack:** TypeScript, React, React Three Fiber (`@react-three/fiber`), `@react-three/drei` (`OrbitControls`), vitest, vite, pnpm.

## Global Constraints

- Package manager is **pnpm**. Frontend only: no Rust, ingest, brain, or backend changes; the `RadarAgent` data contract is unchanged.
- FSD import direction: `app -> views -> modules -> shared`, no sibling-module imports, enforced by `pnpm check:arch`. Use the `@/` alias (`@` maps to `web/`).
- Honest-viz: node position maps to real folder (cwd) plus hierarchy (parentId/depth), size to real `contextTokens` (`radarRadius`), glow to real liveness/status. Never fabricate a signal.
- No em dashes or en dashes anywhere in artifacts (code, comments, docs, commits). Use commas, colons, parentheses. Avoid marketing words.
- Tests: **vitest**, run with `pnpm test` (equals `vitest run`). Config lives in `vite.config.ts` (`test.environment: 'node'`, `include: ['web/**/*.test.ts','web/**/*.test.tsx']`). Colocate `*.test.ts` next to source. Run one file: `pnpm test <path>`.
- The camera lock is radar-only. The Habits tab shares the rig and must keep its current camera (rotate + pan).
- Verify before claiming done: run `pnpm test`, `pnpm build`, `pnpm check:arch` and read the real output.
- The live `pnpm tauri dev` from this session uses vite port **1420**. For the mock harness use the `warden-preview` launch config (port **1421**) so the two do not clash.

## File map

- `web/viz/modules/radar/radarLayout.ts` (MODIFY): rewrite the placement section of `layoutRadarScene` (Tasks 1-4). Keep signature and topology prologue.
- `web/viz/modules/radar/radarLayout.test.ts` (MODIFY): replace orbital-placement assertions with rail-board assertions (Tasks 1-3).
- `web/viz/shared/scene/CameraRig.tsx` (MODIFY): remove `FlyControls`/`flyMode`, add `locked` prop, straight-on overview + locked `OrbitControls` (Task 5).
- `web/viz/modules/radar/RadarConstellation.tsx` (MODIFY): remove `useNodeDrag` + `overrides`/`onMoveNode` from the radar path (Task 6); wire folder-tag click (Task 7).
- `web/viz/shared/scene/useNodeDrag.ts` (DELETE): radar-only (Task 6).
- `web/viz/views/war-room/WarRoom.tsx` (MODIFY): drop `flyMode`/`F`-key, drop radar `overrides`/`onMoveNode`, pass `locked`, add fit-chip + Esc-to-fit, remove `☰`/`Sidebar`, pass `showLedger` (Tasks 5-9).
- `web/viz/views/war-room/chrome.tsx` (MODIFY): add `showLedger` gate around the ledger toggle + dock (Task 9).
- `web/viz/views/war-room/Sidebar.tsx`, `web/viz/modules/radar/rosterTree.ts` (DELETE if unused after Task 8).
- `web/style.css` (MODIFY): remove `.wd-side-toggle` rules (Task 8). Keep `.wd-filterbar` and `.wd-ledger*`.

---

### Task 1: Rail board layout (grouping, rails, beads, simple subtrees)

Rewrite the placement half of `layoutRadarScene`. Keep everything from the function top through the `roots` filter (the topology prologue: `byId`, `resolvesParent`, `childrenOf`, `roots`, `nodes`, `links`). Replace the orbital `placeChildren` + folder-constellation-arc placement (from the `placeChildren` definition through the `return`) with the rail placement below. Signature and return type stay `layoutRadarScene(model: RadarSceneModel): RadarLayout` returning `{ nodes, links, clusters }`.

**Files:**
- Modify: `web/viz/modules/radar/radarLayout.ts` (placement section of `layoutRadarScene`)
- Test: `web/viz/modules/radar/radarLayout.test.ts`

**Interfaces:**
- Consumes: `RadarSceneModel` (`{ generatedAt: string; agents: RadarAgent[] }`), `RadarAgent`, `LayoutNode` (`{ id, kind, position: {x,y,z}, radius, agentId, harness, radarAgent?, depth? }`), `OrbLink` (`{ source, target, kind: 'agent_issue' }`), `RadarCluster` (`{ key, label, harness, center: Vec3, radius }`). Reuses in-file helpers `radarRadius`, `makeNode`, `radarHarness`, `isFlatAgent` and the prologue's `childrenOf`/`roots`.
- Produces: `layoutRadarScene(model): RadarLayout` with all nodes at `z === 0`, one rail per folder.

- [ ] **Step 1: Write failing tests for the rail board**

Append these tests to `web/viz/modules/radar/radarLayout.test.ts` (they reuse the existing `agent()` fixture). Add a fixture for a two-folder, one-subtree model at the top of the new `describe`:

```ts
function twoFolders(): RadarSceneModel {
  return {
    generatedAt: 'T0',
    agents: [
      // folder A (cwd "alpha"): two roots, one with a child + grandchild
      agent({ id: 'a1', depth: 0, parentId: null, cwd: 'alpha', contextTokens: 120000, childCount: 1 }),
      agent({ id: 'a1-c', depth: 1, parentId: 'a1', cwd: 'alpha', contextTokens: 8000, childCount: 1 }),
      agent({ id: 'a1-gc', depth: 2, parentId: 'a1-c', cwd: 'alpha', contextTokens: 3000 }),
      agent({ id: 'a2', depth: 0, parentId: null, cwd: 'alpha', contextTokens: 40000 }),
      // folder B (cwd "beta"): one root
      agent({ id: 'b1', depth: 0, parentId: null, cwd: 'beta', contextTokens: 60000 }),
    ],
  };
}

describe('layoutRadarScene abacus board', () => {
  it('places every node on the board plane (z = 0)', () => {
    const layout = layoutRadarScene(twoFolders());
    for (const n of layout.nodes) expect(n.position.z).toBe(0);
  });

  it('gives each folder its own rail (distinct y), ordered top to bottom', () => {
    const layout = layoutRadarScene(twoFolders());
    const railYalpha = layout.nodes.find((n) => n.id === 'a1')!.position.y;
    const railYbeta = layout.nodes.find((n) => n.id === 'b1')!.position.y;
    expect(railYalpha).not.toBe(railYbeta);
    // folder "alpha" sorts before "beta", so alpha is the top rail (greater y)
    expect(railYalpha).toBeGreaterThan(railYbeta);
    // both roots of "alpha" share the alpha rail y
    expect(layout.nodes.find((n) => n.id === 'a2')!.position.y).toBe(railYalpha);
  });

  it('orders root beads left to right on their rail', () => {
    const layout = layoutRadarScene(twoFolders());
    const a1 = layout.nodes.find((n) => n.id === 'a1')!;
    const a2 = layout.nodes.find((n) => n.id === 'a2')!;
    expect(a1.position.x).toBeLessThan(a2.position.x);
  });

  it('hangs subagents one row-step below their parent per depth level', () => {
    const layout = layoutRadarScene(twoFolders());
    const a1 = layout.nodes.find((n) => n.id === 'a1')!;
    const c = layout.nodes.find((n) => n.id === 'a1-c')!;
    const gc = layout.nodes.find((n) => n.id === 'a1-gc')!;
    expect(c.position.y).toBeLessThan(a1.position.y);
    expect(gc.position.y).toBeLessThan(c.position.y);
    // equal steps per level
    expect(a1.position.y - c.position.y).toBeCloseTo(c.position.y - gc.position.y, 5);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `pnpm test web/viz/modules/radar/radarLayout.test.ts`
Expected: the four new tests FAIL (nodes are on the old orbital layout, not z=0 rails).

- [ ] **Step 3: Rewrite the placement section**

In `web/viz/modules/radar/radarLayout.ts`, keep the function body up to and including the `roots` filter and the `const nodes` / `const links` declarations. Delete from the multi-shell child placement comment and its `placeChildren` function through the final `return { nodes, links, clusters };`. Replace with:

```ts
  // abacus rails: one horizontal rail per folder, stacked top to bottom
  const RAIL_GAP = 3.2; // base vertical space below a rail (before depth adjust)
  const ROW_STEP = 1.5; // vertical drop per subagent depth level
  const BEAD_GAP = 1.6; // min horizontal space after a root bead
  const SIB_GAP = 1.0; // min horizontal space between sibling subagents

  const folderKey = (r: RadarAgent): string => {
    const dir = r.cwd?.trim();
    if (dir) return `dir:${dir}`;
    const label = r.label?.trim();
    if (label) return `task:${label}`;
    return `harness:${r.harness || 'none'}`;
  };
  const folderLabelOf = (r: RadarAgent): string =>
    r.cwd?.trim() || r.label?.trim() || radarHarness(r.harness).label;

  // Group roots into rails. `roots` is already id-sorted (deterministic); a rail
  // appears in the order its first root appears, and holds its members in that
  // same order. Position never depends on activity, so a folder never jumps when
  // an agent inside it changes state (spatial stability).
  const railOrder: string[] = [];
  const railMembers = new Map<string, RadarAgent[]>();
  for (const r of roots) {
    const k = folderKey(r);
    if (!railMembers.has(k)) {
      railMembers.set(k, []);
      railOrder.push(k);
    }
    railMembers.get(k)!.push(r);
  }

  // Subtree: place each child one ROW_STEP below its parent, siblings fanned
  // horizontally and centred on the parent's x. (Task 2 makes this width-aware.)
  function placeSubtree(parent: RadarAgent, px: number, py: number) {
    const kids = childrenOf.get(parent.id);
    if (!kids || kids.length === 0) return;
    const cy = py - ROW_STEP;
    const span = (kids.length - 1) * SIB_GAP;
    kids.forEach((kid, i) => {
      const cx = px - span / 2 + i * SIB_GAP;
      const node = makeNode(kid, { x: cx, y: cy, z: 0 });
      nodes.push(node);
      links.push({ source: parent.id, target: kid.id, kind: 'agent_issue' });
      placeSubtree(kid, cx, cy);
    });
  }

  const clusters: RadarCluster[] = [];
  let railY = 0;
  for (const k of railOrder) {
    const members = railMembers.get(k)!;
    let x = 0;
    for (const root of members) {
      const rootNode = makeNode(root, { x, y: railY, z: 0 });
      nodes.push(rootNode);
      placeSubtree(root, x, railY);
      x += 2 * rootNode.radius + BEAD_GAP;
    }
    // Folder tag sits at the rail head, just left of the first bead.
    clusters.push({
      key: k,
      label: folderLabelOf(members[0]),
      harness: members[0].harness,
      center: { x: -BEAD_GAP, y: railY, z: 0 },
      radius: 1,
    });
    railY -= RAIL_GAP;
  }

  return { nodes, links, clusters };
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `pnpm test web/viz/modules/radar/radarLayout.test.ts`
Expected: the four new tests PASS. Some OLD tests that assert orbital geometry (for example ones referencing `TILT_Y`/`TILT_Z`, ring distances, or the cluster arc) will now FAIL. That is expected; fix them in Step 5.

- [ ] **Step 5: Update or remove the stale orbital tests**

Open `web/viz/modules/radar/radarLayout.test.ts`. For each failing test that asserted orbital placement (moon distances, tilt, cluster arc depth), either delete it or rewrite it as a board assertion. Keep and do NOT change: `radarRadius` tests, the "mains bigger than subs" test, the "one parent to child link per non-root" test, and the "deterministic for the same model" test (these still hold on the board). If `TILT_Y`/`TILT_Z` are no longer exported after Task 1, remove them from the import on line 2.

Run: `pnpm test web/viz/modules/radar/radarLayout.test.ts`
Expected: PASS (all).

- [ ] **Step 6: Typecheck and commit**

Run: `pnpm build`
Expected: `tsc` passes (no type errors), vite bundles.

```bash
git add web/viz/modules/radar/radarLayout.ts web/viz/modules/radar/radarLayout.test.ts
git commit -m "feat(radar): rail-board layout, folders as rails and agents as beads"
```

---

### Task 2: Width-aware subtrees and bead packing

Make `placeSubtree` a tidy tree: a subtree reserves horizontal width so sibling subtrees do not overlap, children are centred under their parent, and each root bead advances x by its whole subtree width so the next bead clears it.

**Files:**
- Modify: `web/viz/modules/radar/radarLayout.ts`
- Test: `web/viz/modules/radar/radarLayout.test.ts`

**Interfaces:**
- Consumes: same as Task 1.
- Produces: `subtreeWidth(agent): number` helper; `placeSubtree` now centres children and returns the parent x; root x-advance uses `subtreeWidth`.

- [ ] **Step 1: Write failing tests**

Add to the `describe('layoutRadarScene abacus board', ...)` block:

```ts
  it('centres a single child under its parent', () => {
    const layout = layoutRadarScene(twoFolders());
    const c = layout.nodes.find((n) => n.id === 'a1-c')!;
    const a1 = layout.nodes.find((n) => n.id === 'a1')!;
    expect(c.position.x).toBeCloseTo(a1.position.x, 5);
  });

  it('centres multiple children on the mean of their parent x', () => {
    const model: RadarSceneModel = {
      generatedAt: 'T0',
      agents: [
        agent({ id: 'p', depth: 0, parentId: null, cwd: 'alpha', childCount: 2 }),
        agent({ id: 'p-c1', depth: 1, parentId: 'p', cwd: 'alpha' }),
        agent({ id: 'p-c2', depth: 1, parentId: 'p', cwd: 'alpha' }),
      ],
    };
    const layout = layoutRadarScene(model);
    const p = layout.nodes.find((n) => n.id === 'p')!;
    const c1 = layout.nodes.find((n) => n.id === 'p-c1')!;
    const c2 = layout.nodes.find((n) => n.id === 'p-c2')!;
    expect((c1.position.x + c2.position.x) / 2).toBeCloseTo(p.position.x, 5);
    expect(c1.position.x).not.toBeCloseTo(c2.position.x, 1); // spread apart
  });

  it('spaces a root with a wide subtree clear of the next root bead', () => {
    // a1 has a subtree; a2 is a bare root. a2 must sit right of a1's subtree.
    const layout = layoutRadarScene(twoFolders());
    const a1c = layout.nodes.find((n) => n.id === 'a1-c')!;
    const a1gc = layout.nodes.find((n) => n.id === 'a1-gc')!;
    const a2 = layout.nodes.find((n) => n.id === 'a2')!;
    const subtreeRight = Math.max(a1c.position.x, a1gc.position.x);
    expect(a2.position.x).toBeGreaterThan(subtreeRight);
  });
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm test web/viz/modules/radar/radarLayout.test.ts`
Expected: the "wide subtree clear of next bead" test FAILS (Task 1 advanced x by bead width only, ignoring subtree width). The centring tests may already pass for the single/2-child case; keep them.

- [ ] **Step 3: Implement width-aware placement**

In `web/viz/modules/radar/radarLayout.ts`, replace the `placeSubtree` function and the root x-advance from Task 1 with:

```ts
  // Width a subtree needs on the board: a leaf takes its own bead footprint; an
  // internal node takes the max of its own footprint and the summed width of its
  // children (plus sibling gaps). Bottom-up, memoised per agent.
  const widthCache = new Map<string, number>();
  function subtreeWidth(a: RadarAgent): number {
    const cached = widthCache.get(a.id);
    if (cached !== undefined) return cached;
    const own = 2 * radarRadius(a.contextTokens, a.depth) + SIB_GAP;
    const kids = childrenOf.get(a.id);
    let w = own;
    if (kids && kids.length > 0) {
      const childrenW = kids.reduce((s, k) => s + subtreeWidth(k), 0);
      w = Math.max(own, childrenW);
    }
    widthCache.set(a.id, w);
    return w;
  }

  // Place a subtree whose block spans [left, left + subtreeWidth(parent)] at row
  // `py`; the parent is centred over its children (or over its own block if leaf).
  function placeSubtree(parent: RadarAgent, left: number, py: number): number {
    const w = subtreeWidth(parent);
    const kids = childrenOf.get(parent.id);
    let parentX: number;
    if (!kids || kids.length === 0) {
      parentX = left + w / 2;
    } else {
      let cursor = left;
      const cy = py - ROW_STEP;
      const centres: number[] = [];
      for (const kid of kids) {
        const cx = placeSubtree(kid, cursor, cy);
        centres.push(cx);
        cursor += subtreeWidth(kid);
      }
      parentX = centres.reduce((s, c) => s + c, 0) / centres.length;
    }
    const node = makeNode(parent, { x: parentX, y: py, z: 0 });
    nodes.push(node);
    const pid = parent.parentId;
    if (pid && childrenOf.has(pid) && childrenOf.get(pid)!.some((k) => k.id === parent.id)) {
      links.push({ source: pid, target: parent.id, kind: 'agent_issue' });
    }
    return parentX;
  }
```

Then change the per-rail bead loop so each root consumes its subtree block (replace the Task 1 `for (const root of members)` body):

```ts
    let x = 0;
    for (const root of members) {
      placeSubtree(root, x, railY);
      x += subtreeWidth(root) + BEAD_GAP;
    }
```

Note: `placeSubtree` now pushes the root node itself, so remove the `const rootNode = makeNode(...)` line and the separate child `links.push` from the Task 1 loop (both moved inside `placeSubtree`).

- [ ] **Step 4: Run to verify pass**

Run: `pnpm test web/viz/modules/radar/radarLayout.test.ts`
Expected: PASS (all, including the Task 1 tests, the link-count test, and the determinism test).

- [ ] **Step 5: Typecheck and commit**

Run: `pnpm build`
Expected: passes.

```bash
git add web/viz/modules/radar/radarLayout.ts web/viz/modules/radar/radarLayout.test.ts
git commit -m "feat(radar): width-aware subagent trees so beads never collide"
```

---

### Task 3: Depth-adaptive rail spacing

A rail with a deep subtree must leave more room below it so its tree does not crash into the next rail. Make the vertical drop after each rail grow with that rail's deepest subtree.

**Files:**
- Modify: `web/viz/modules/radar/radarLayout.ts`
- Test: `web/viz/modules/radar/radarLayout.test.ts`

**Interfaces:**
- Consumes: same.
- Produces: rail y-drop equals `RAIL_GAP + maxDepthOnRail * ROW_STEP`.

- [ ] **Step 1: Write the failing test**

Add:

```ts
  it('leaves more vertical room below a rail that has a deep subtree', () => {
    // rail "alpha" has depth-2; rail "beta" is flat. Measure the gap under each.
    const model: RadarSceneModel = {
      generatedAt: 'T0',
      agents: [
        agent({ id: 'a', depth: 0, parentId: null, cwd: 'alpha', childCount: 1 }),
        agent({ id: 'a-c', depth: 1, parentId: 'a', cwd: 'alpha', childCount: 1 }),
        agent({ id: 'a-gc', depth: 2, parentId: 'a-c', cwd: 'alpha' }),
        agent({ id: 'b', depth: 0, parentId: null, cwd: 'beta' }),
        agent({ id: 'c', depth: 0, parentId: null, cwd: 'gamma' }),
      ],
    };
    const layout = layoutRadarScene(model);
    const yAlpha = layout.nodes.find((n) => n.id === 'a')!.position.y;
    const yBeta = layout.nodes.find((n) => n.id === 'b')!.position.y;
    const yGamma = layout.nodes.find((n) => n.id === 'c')!.position.y;
    const gapUnderAlpha = yAlpha - yBeta; // alpha has depth 2
    const gapUnderBeta = yBeta - yGamma; // beta is flat
    expect(gapUnderAlpha).toBeGreaterThan(gapUnderBeta);
  });
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm test web/viz/modules/radar/radarLayout.test.ts`
Expected: FAIL (Task 1 used a constant `RAIL_GAP`, so both gaps are equal).

- [ ] **Step 3: Implement adaptive rail drop**

In `web/viz/modules/radar/radarLayout.ts`, add a depth helper before the rail loop:

```ts
  // Deepest subtree depth (relative to the rail) among a rail's members.
  const relDepth = (a: RadarAgent): number => {
    const kids = childrenOf.get(a.id);
    if (!kids || kids.length === 0) return 0;
    return 1 + Math.max(...kids.map(relDepth));
  };
  const railDepth = (members: RadarAgent[]): number => Math.max(0, ...members.map(relDepth));
```

Change the rail loop's advance from `railY -= RAIL_GAP;` to:

```ts
    railY -= RAIL_GAP + railDepth(members) * ROW_STEP;
```

- [ ] **Step 4: Run to verify pass**

Run: `pnpm test web/viz/modules/radar/radarLayout.test.ts`
Expected: PASS (all).

- [ ] **Step 5: Commit**

```bash
git add web/viz/modules/radar/radarLayout.ts web/viz/modules/radar/radarLayout.test.ts
git commit -m "feat(radar): rail spacing adapts to deepest subtree so trees never overlap"
```

---

### Task 4: Folder tags at rail heads + first live look

The board now computes. Render it in the mock harness, confirm the folder tags sit at the rail heads, and adjust the cluster label layer if it still places labels below constellations.

**Files:**
- Modify (if needed): `web/viz/modules/radar/RadarConstellation.tsx` (the `RadarClusterLabels` label placement)
- Verify: `radar-lab.html` in the browser

- [ ] **Step 1: Start the mock harness**

Use the preview tooling with the `warden-preview` config (port 1421, clear of the live app on 1420). Start the server, then open `http://127.0.0.1:1421/radar-lab.html`.

- [ ] **Step 2: Look at the board**

Take a screenshot. Expected: horizontal rails, one per folder from the `RAW_FOREST` mock (`WARDEN`, `JB Hunting`, `webapp`, and the unknown-harness `side-experiment`), root beads left to right, the Claude depth-2 tree (`WARDEN` root, then `Explore`, then `Explore`) branching downward, the Codex `JB Hunting` root with its two explorer moons below it, and the flat `webapp` plus its promoted stray as separate beads. Confirm nothing overlaps.

- [ ] **Step 3: Fix folder-tag placement if wrong**

If the folder labels do not sit at the left head of each rail, open `RadarConstellation.tsx`, find `RadarClusterLabels` (it reads `cluster.center` / `cluster.radius`), and change it to render each label anchored at `cluster.center` (which Task 1 set to the rail head: `{ x: -BEAD_GAP, y: railY, z: 0 }`). Reload and re-screenshot until each tag reads as the rail's folder name at its left edge.

- [ ] **Step 4: Commit (only if RadarConstellation changed)**

```bash
git add web/viz/modules/radar/RadarConstellation.tsx
git commit -m "feat(radar): anchor folder tags at rail heads"
```

---

### Task 5: Lock the camera to the board

Remove free-fly, add a tab-aware `locked` prop, and lock rotation and pan while keeping zoom-to-cursor. When locked, the overview looks straight on at the board.

**Files:**
- Modify: `web/viz/shared/scene/CameraRig.tsx`
- Modify: `web/viz/views/war-room/WarRoom.tsx`
- Verify: `radar-lab.html`

**Interfaces:**
- Consumes: `CameraRig` prop `locked?: boolean` (default `false`).
- Produces: locked `OrbitControls` (no rotate, no pan, zoom-to-cursor). `flyMode` removed from `CameraRig` and `WarRoom`.

- [ ] **Step 1: Remove FlyControls and add `locked` to CameraRig**

In `web/viz/shared/scene/CameraRig.tsx`: on line 27 change the import to drop `FlyControls`:

```tsx
import { OrbitControls } from '@react-three/drei';
```

Replace the props destructure (lines 61-75) with (drop `flyMode`, add `locked`):

```tsx
export function CameraRig({
  selected,
  focusBounds = null,
  homeSignal = 0,
  sceneBounds = null,
  locked = false,
}: {
  selected: LayoutNode | null;
  focusBounds?: Bounds | null;
  homeSignal?: number;
  /** Bounding sphere of the whole active forest; scales zoom range + framing. */
  sceneBounds?: Bounds | null;
  /** Radar board: lock rotate + pan, keep zoom-to-cursor, look straight on. */
  locked?: boolean;
}) {
```

Delete the `if (flyMode) { ... return <FlyControls .../>; }` block (lines 330-335).

- [ ] **Step 2: Lock the OrbitControls block**

Replace the `<OrbitControls .../>` return (lines 337-357) with:

```tsx
  return (
    <OrbitControls
      ref={controls}
      makeDefault
      enableDamping
      dampingFactor={0.15}
      rotateSpeed={0.95}
      zoomSpeed={1.0}
      // Board is locked: no rotate, no pan; the wheel still dollies toward the
      // cursor. Habits keeps the uncaged rig (rotate + pan).
      enableRotate={!locked}
      enablePan={!locked}
      screenSpacePanning={!locked}
      zoomToCursor
      minDistance={MIN_DIST}
      maxDistance={fit.maxDist}
      minPolarAngle={locked ? Math.PI / 2 : 0.01}
      maxPolarAngle={locked ? Math.PI / 2 : Math.PI - 0.01}
      minAzimuthAngle={locked ? 0 : -Infinity}
      maxAzimuthAngle={locked ? 0 : Infinity}
    />
  );
```

- [ ] **Step 3: Point the locked overview straight on**

Still in `CameraRig.tsx`, find the overview direction constant `OVERVIEW_DIR` (near line 33; today it is the 3/4 hero angle `(0.35, 0.28, 1)`). Where the rig computes the overview/home camera position from `OVERVIEW_DIR`, use a straight-on direction when `locked`:

```tsx
  const overviewDir = locked ? new THREE.Vector3(0, 0, 1) : OVERVIEW_DIR;
```

Use `overviewDir` in place of `OVERVIEW_DIR` in the overview/home position math (the `homeSignal` reset and the initial fit). If `OVERVIEW_DIR` is a bare tuple, adapt to however position is currently derived; the goal is: locked means camera on +Z looking at board centre, up +Y.

- [ ] **Step 4: Thread `locked` from WarRoom and drop flyMode**

In `web/viz/views/war-room/WarRoom.tsx`:
- Delete the `flyMode` state + `F`-key handler (lines 516-533).
- Remove `flyMode={flyMode}` from the scene props (it appears in the inner wrapper around line 344 and the scene-body mount around line 885). Add `locked={displayTab === 'radar'}` to the `CameraRig` mount (in `SceneShell`).
- Remove the now-unused `flyMode` from the scene-body wrapper's prop declarations (lines 273-275 destructure, 297-301 types).

Search the file for any remaining `flyMode` references after this and delete them.

- [ ] **Step 5: Typecheck**

Run: `pnpm build`
Expected: passes, no unused-symbol or missing-prop errors.

- [ ] **Step 6: Verify live**

In `radar-lab.html` (port 1421): try to click-drag the scene. Expected: it does NOT rotate and does NOT pan. Scroll: it zooms toward the cursor. Confirm the board is seen straight on (rails horizontal, no perspective tilt between rails). Screenshot.

- [ ] **Step 7: Commit**

```bash
git add web/viz/shared/scene/CameraRig.tsx web/viz/views/war-room/WarRoom.tsx
git commit -m "feat(radar): lock camera to the board (zoom-to-cursor, no rotate/pan/fly)"
```

---

### Task 6: Remove node dragging

Drop the radar-only drag hook and its wiring. Keep `positionOverrides.ts` (shared with the Habits layout).

**Files:**
- Modify: `web/viz/modules/radar/RadarConstellation.tsx`
- Modify: `web/viz/views/war-room/WarRoom.tsx`
- Delete: `web/viz/shared/scene/useNodeDrag.ts`
- Verify: `radar-lab.html`

- [ ] **Step 1: Strip drag from RadarConstellation**

In `web/viz/modules/radar/RadarConstellation.tsx`:
- Remove the import on line 34 (`import { useNodeDrag, type DragApi } from '@/viz/shared/scene/useNodeDrag';`).
- Remove `const drag = useNodeDrag(onMoveNode ?? (() => {}));` (around line 807).
- In the `layout` memo (around line 800-803), drop the override wrap so it reads:

```tsx
  const layout = useMemo(() => layoutRadarScene(layoutModel), [layoutModel]);
```

- Remove the `overrides` and `onMoveNode` props from the `RadarForest` component's prop type and destructure. Remove `drag` from wherever it is passed into `RadarGlobe`, and in `RadarGlobe` remove the pointer-down `drag?.begin(...)` call and the `movedRef` select-skip so a plain click selects again. (Search the file for `drag` and `onMoveNode` and remove each remaining reference.)

- [ ] **Step 2: Strip radar drag from WarRoom**

In `web/viz/views/war-room/WarRoom.tsx`:
- Remove `onMoveNode` (lines 417-424) and the `onMoveNode={onMoveNode}` + `overrides={overrides}` props passed to `RadarForest` (lines 360-361) and to the scene-body mount (lines 886-887).
- In the radar layout memo (line 486), drop the override wrap:

```tsx
  const radarLayout = useMemo(() => layoutRadarScene(radarModel), [radarModel]);
```

Leave the Habits `layout` memo on line 479 (`applyLayoutOverrides(layoutOrbScene(model), overrides)`) and the `overrides` state untouched: they remain shared infra. Leave the double-click home reset (lines 597-603) as-is; it is the fit/home gesture reused in Task 7. If `overrides` is now unused by the radar path only, that is fine; do not delete the state (Habits reads it).

- [ ] **Step 3: Delete the hook**

```bash
git rm web/viz/shared/scene/useNodeDrag.ts
```

- [ ] **Step 4: Typecheck**

Run: `pnpm build`
Expected: passes. If `DragApi` or `useNodeDrag` is referenced anywhere else, the compiler will flag it; remove those references.

- [ ] **Step 5: Verify live**

In `radar-lab.html`: click-drag a bead. Expected: the bead does NOT move. A single click on a bead selects it (detail panel opens). Screenshot.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(radar): remove node dragging from the locked board"
```

---

### Task 7: Click-to-focus and fit-to-overview

Bead click already frames + opens detail (the select-to-focus effect). Add: folder-tag click frames that rail, and Escape or a fit-chip or clicking empty space eases back to the framed overview (the existing `homeSignal`).

**Files:**
- Modify: `web/viz/views/war-room/WarRoom.tsx`
- Modify: `web/viz/modules/radar/RadarConstellation.tsx` (folder-tag click)
- Modify: `web/style.css` (fit-chip)
- Verify: `radar-lab.html`

**Interfaces:**
- Consumes: `setHomeSignal` (exists), `focusBounds` prop on `CameraRig` (exists), `enclosingBounds` from `web/viz/shared/scene/cameraFraming.ts`.
- Produces: `onPickFolder(folderKey)` frames that folder; Escape and fit-chip bump `homeSignal`.

- [ ] **Step 1: Escape and fit-chip to overview**

In `web/viz/views/war-room/WarRoom.tsx`, add a keydown effect (replacing the removed `F` handler) that on `Escape`, when `displayTab === 'radar'` and not typing, bumps the home signal:

```tsx
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const el = document.activeElement as HTMLElement | null;
      const typing =
        !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
      if (typing) return;
      if (e.key === 'Escape' && displayTab === 'radar') {
        e.preventDefault();
        setHomeSignal((s) => s + 1);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [displayTab]);
```

Add a small fit-chip button, radar-only, that calls the same `setHomeSignal((s) => s + 1)` (place it near the filter mount around line 954):

```tsx
      {displayTab === 'radar' ? (
        <button
          type="button"
          className="wd-fit-chip"
          title="Fit to overview"
          onClick={() => setHomeSignal((s) => s + 1)}
        >
          ⤢ fit
        </button>
      ) : null}
```

Add styling for `.wd-fit-chip` in `web/style.css` (reuse the pill look; sit it bottom-right where the ledger used to be):

```css
.wd-fit-chip {
  position: absolute;
  right: var(--side-margin);
  bottom: 18px;
  z-index: var(--z-controls);
  padding: 6px 12px;
  border-radius: 10px;
  background: var(--panel-strong);
  border: 1px solid var(--hair);
  color: var(--ink-soft);
  font-size: 10px;
  letter-spacing: 0.12em;
  text-transform: uppercase;
  cursor: pointer;
  backdrop-filter: blur(11px);
  -webkit-backdrop-filter: blur(11px);
}
.wd-fit-chip:hover { color: var(--green); border-color: var(--hair-bright); }
```

- [ ] **Step 2: Folder-tag click frames the rail**

In `web/viz/modules/radar/RadarConstellation.tsx`, give `RadarClusterLabels` an `onPick(folderKey)` prop and call it on label click. In `WarRoom.tsx`, pass a handler that frames that folder's nodes:

```tsx
  const onPickFolder = useCallback((key: string) => {
    const cluster = radarLayout.clusters.find((c) => c.key === key);
    if (!cluster) return;
    const members = radarLayout.nodes.filter((n) => Math.abs(n.position.y - cluster.center.y) < 0.01);
    const pts = members.map((n) => n.position);
    if (pts.length) setFocusBounds(enclosingBounds(pts));
  }, [radarLayout]);
```

Confirm `setFocusBounds` exists (the `focusBounds` state feeding `CameraRig`); if the current code drives `focusBounds` only via the focus stack, add a `const [focusBounds, setFocusBounds] = useState<Bounds | null>(null);` and pass `focusBounds={focusBounds}` to `CameraRig` (it already accepts the prop). Import `enclosingBounds` from `@/viz/shared/scene/cameraFraming`.

- [ ] **Step 3: Typecheck**

Run: `pnpm build`
Expected: passes.

- [ ] **Step 4: Verify live**

In `radar-lab.html`: click a folder tag, the camera glides to frame that rail. Press Escape, it eases back to the whole board. Click the fit-chip, same. Click a bead, it frames the bead + detail panel. Screenshot each.

- [ ] **Step 5: Commit**

```bash
git add web/viz/views/war-room/WarRoom.tsx web/viz/modules/radar/RadarConstellation.tsx web/style.css
git commit -m "feat(radar): click a rail to focus it, Escape or fit-chip returns to overview"
```

---

### Task 8: Remove the hamburger and roster sidebar

**Files:**
- Modify: `web/viz/views/war-room/WarRoom.tsx`
- Modify: `web/style.css`
- Delete (if unused): `web/viz/views/war-room/Sidebar.tsx`, `web/viz/modules/radar/rosterTree.ts`
- Verify: `radar-lab.html` / full app

- [ ] **Step 1: Remove the button, state, and mount**

In `web/viz/views/war-room/WarRoom.tsx`:
- Delete the `☰` button JSX (lines 927-940).
- Delete `const [sidebarOpen, setSidebarOpen] = useState(false);` (line 444).
- Delete `const onToggleSidebar = useCallback(() => setSidebarOpen((o) => !o), []);` (line 614).
- Delete the `<Sidebar .../>` mount (lines 942-950).
- Remove the `Sidebar` import and any now-unused roster values (`rosterGroups`, `rosterHeader`, `onPickRoster`, the `buildRadarRoster` import) if they are only used by the removed sidebar. The compiler will flag leftovers.

- [ ] **Step 2: Remove the button styles**

In `web/style.css`, delete the `.wd-side-toggle` rules (lines 1484-1508, including `:hover`, `:focus-visible`, `.is-open`).

- [ ] **Step 3: Check for other importers, then delete dead files**

```bash
grep -rn "Sidebar" web/viz --include=*.ts --include=*.tsx | grep -v "war-room/Sidebar.tsx"
grep -rn "rosterTree\|buildRadarRoster" web/viz --include=*.ts --include=*.tsx
```

If the only hits are the files we already edited, delete them:

```bash
git rm web/viz/views/war-room/Sidebar.tsx web/viz/modules/radar/rosterTree.ts
```

If `rosterTree.ts` has a colocated test or another importer, keep the file unused; do not force the delete.

- [ ] **Step 4: Typecheck and arch check**

Run: `pnpm build && pnpm check:arch`
Expected: both pass.

- [ ] **Step 5: Verify live**

In `radar-lab.html`: no `☰` button top-left; the board is clean; clicking a bead still selects it. Screenshot.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(radar): remove roster hamburger and sidebar, select on the board"
```

---

### Task 9: Hide the ledger on the radar tab

**Files:**
- Modify: `web/viz/views/war-room/chrome.tsx`
- Modify: `web/viz/views/war-room/WarRoom.tsx`
- Verify: `radar-lab.html` / full app

**Interfaces:**
- Consumes: new `Chrome` prop `showLedger: boolean`.
- Produces: ledger toggle + dock render only when `showLedger`.

- [ ] **Step 1: Add the `showLedger` prop to Chrome**

In `web/viz/views/war-room/chrome.tsx`, add `showLedger` to the destructure and to the props type (near lines 629-681):

```tsx
  showLedger,
```
```tsx
  showLedger: boolean;
```

Gate the ledger toggle + dock (lines 719-743) so both render only when `showLedger`:

```tsx
      {showLedger ? (
        <>
          <button
            type="button"
            className={`wd-ledger-toggle${ledgerOpen ? ' is-open' : ''}`}
            aria-expanded={ledgerOpen}
            aria-controls="wd-ledger"
            title="Guardrail ledger"
            onClick={onToggleLedger}
          >
            <span className="wd-ledger-toggle-glyph" aria-hidden="true">⬢</span>
            LEDGER
            {ledgerCount > 0 ? <span className="wd-ledger-toggle-count">{ledgerCount}</span> : null}
          </button>
          <div className={`wd-ledger-dock${ledgerOpen ? ' is-open' : ''}`} id="wd-ledger">
            {ledgerOpen ? (
              <Ledger
                artifacts={artifacts}
                reverting={reverting}
                onRevert={onRevertFix}
                onClose={onToggleLedger}
              />
            ) : null}
          </div>
        </>
      ) : null}
```

- [ ] **Step 2: Pass `showLedger` from WarRoom**

In `web/viz/views/war-room/WarRoom.tsx`, add to the `<Chrome .../>` mount (lines 960-985):

```tsx
        showLedger={displayTab !== 'radar'}
```

- [ ] **Step 3: Typecheck**

Run: `pnpm build`
Expected: passes.

- [ ] **Step 4: Verify live**

Radar tab: no `LEDGER` toggle bottom-right; the filter reads as the single, centred bottom element. Switch to Habits (in the full app or `dev-warroom.html`): the ledger toggle is still there. Screenshot the radar tab.

- [ ] **Step 5: Commit**

```bash
git add web/viz/views/war-room/chrome.tsx web/viz/views/war-room/WarRoom.tsx
git commit -m "feat(radar): hide the guardrail ledger on the radar tab"
```

---

### Task 10: Full verification and cleanup

**Files:**
- Verify only, plus optional dead-code removal.

- [ ] **Step 1: Full test + build + arch gates**

Run: `pnpm test`
Expected: all suites PASS.

Run: `pnpm build`
Expected: `tsc` clean, vite bundles.

Run: `pnpm check:arch`
Expected: PASS (no import-boundary violations).

- [ ] **Step 2: Real app capture**

Rebuild the live app so it picks up the changes (the session's `pnpm tauri dev` hot-reloads the frontend; if not, restart it). Summon the war-room and capture the radar tab (screenshot the WARDEN window). Expected: rails per folder, beads with downward subtrees, no ledger, no hamburger, centred filter, straight-on locked camera. Confirm zoom-to-cursor and click-to-focus work in the real window.

- [ ] **Step 3: Optional cleanup**

If the old orbital helpers in `radarLayout.ts` (for example `ringPosition`, `orbitRadius`, `localRingRadius`, `shellCapacity`, `angleSeed`, `TILT_Y`, `TILT_Z`, `CLUSTER_*` constants) are now unused, remove them and their exports. Run `pnpm build` to confirm nothing else referenced them. Commit:

```bash
git add -A
git commit -m "chore(radar): drop unused orbital-layout helpers"
```

- [ ] **Step 4: Final branch state**

Confirm the branch `radar-abacus-redesign` holds the spec commit plus the task commits, working tree clean:

```bash
git status -sb
git log --oneline -12
```

Do NOT push or open a PR without Karim's explicit instruction.

---

## Self-review

**Spec coverage:**
- Remove ledger (radar), Task 9. Remove hamburger + roster, Task 8. Center filter, already centered (verified Task 9; noted in File map). Globes kept and re-placed, Tasks 1-4. Switcher untouched. Detail panel + hover untouched.
- Rail board (rails=folders, beads=agents, subagents downward, adaptive spacing, stable order), Tasks 1-3. Folder tags at rail heads, Task 4.
- Locked camera (no rotate/pan/fly/node-drag, keep zoom-to-cursor, straight-on), Tasks 5 and 6. Click-to-focus + fit, Task 7.
- Tab-aware lock (Habits unaffected), Task 5 (`locked` prop) and Task 9 (ledger only on non-radar).
- Scale/overflow (min glyph floor, vertical paging): NOT separately tasked; these are tuning atop the working board. Left as follow-up if the live capture in Task 10 shows a legibility problem with many folders. Flagged here rather than hidden.
- Honest-viz preserved: Tasks 1-2 reuse `radarRadius` (size), `radarAgent`/`depth` (hierarchy), harness (color); the glow path in the scene is untouched.
- Prototype in `radar-lab.html`: Tasks 4-9 verify there (port 1421).

**Placeholder scan:** No "TBD"/"handle edge cases" steps. The three soft spots (CameraRig overview-direction internals in Task 5 Step 3; `RadarClusterLabels` placement in Task 4 Step 3; folder-tag click wiring in Task 7 Step 2) each name the exact file, the exact target behavior, and a browser check, because their surrounding code was not extracted verbatim. They are scoped edits with a concrete acceptance, not vague directives.

**Type consistency:** `layoutRadarScene(model: RadarSceneModel): RadarLayout` signature unchanged across all tasks. `LayoutNode.position` is `{x,y,z}`; tests read `.position.x/.y/.z`. Links stay `{ source, target, kind: 'agent_issue' }`. `CameraRig` prop is `locked` (added) with `flyMode` removed in the same task, so no caller references a dropped prop after Task 5. `showLedger: boolean` added to `Chrome` and passed in the same task pair (9).
