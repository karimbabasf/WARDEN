// WarRoom.tsx: the whole interface. One persistent 3D scene (the void, a free-orbit
// camera locked straight-on, and the live RADAR agent forest) with a thin DOM chrome
// layer over it: a brand mark, the harness filter, the focus breadcrumb, the selected
// agent's detail panel, and an honest empty state. Every globe/flare maps to a real
// signal from the backend `radar_state` (session liveness, context weight, subagent
// hierarchy); nothing is fabricated.

import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from 'react';
import { Canvas, useThree } from '@react-three/fiber';
import { EffectComposer, Bloom, Vignette } from '@react-three/postprocessing';
import { Environment, Lightformer } from '@react-three/drei';
import * as THREE from 'three';
import { invoke } from '@tauri-apps/api/core';
import type { Bridge, SceneState } from '@/viz/shared/state/bridge';
import { StarCatalog } from '@/viz/shared/scene/StarCatalog';
import { CameraRig } from '@/viz/shared/scene/CameraRig';
import { RadarForest } from '@/viz/modules/radar/RadarConstellation';
import { RadarDetailPanel } from '@/viz/modules/radar/RadarDetailPanel';
import { FilterBar } from './FilterBar';
import { Breadcrumb } from './Breadcrumb';
import { layoutRadarScene, isFlatAgent } from '@/viz/modules/radar/radarLayout';
import type { RadarAgent, RadarSceneModel } from '@/viz/shared/types/radarTypes';
import type { EmphasisFilter } from '@/viz/shared/lib/emphasis';
import type { LayoutNode } from '@/viz/shared/types/orbTypes';
import { subtreeBounds, enclosingBounds, type Bounds } from '@/viz/shared/scene/cameraFraming';
import { frameloopFor } from '@/viz/shared/scene/frameloop';

const BG = '#020403';

export const RADAR_VISIBLE_PULL_MS = 750;

// The render loop runs whenever the window is on screen (even unfocused or on another
// display). The ONLY thing that pauses it is MINIMIZE (CPU saver). A summoned window
// is active regardless of the page-visibility flag (a native .show() may leave
// document.hidden stale-true). Dev/browser (no summon) keys off page visibility so a
// hidden tab still pauses.
export function activeFor(
  summoned: boolean | undefined,
  visHidden: boolean,
  minimized = false,
): boolean {
  if (minimized) return false;
  return Boolean(summoned) || !visHidden;
}

export function isDiscoveryHomeDoubleClickAllowed({
  selectedId,
  focusDepth,
  eventTarget,
}: {
  selectedId: string | null;
  focusDepth: number;
  eventTarget: EventTarget | null;
}): boolean {
  if (selectedId !== null || focusDepth > 0) return false;
  if (typeof Element === 'undefined' || !(eventTarget instanceof Element)) return true;
  return eventTarget.closest('button, input, select, textarea, a, [contenteditable="true"], [role="button"]') === null;
}

// The persistent scene shell: the single always-mounted void (background, fog,
// lights, Environment, starfield, the shared free-orbit CameraRig and the post
// stack) wrapped around the live radar forest. The camera is locked straight-on for
// the radar board; Escape and empty-click ease it back to the framed overview.
function SceneShell({
  radarModel,
  selected,
  selectedId,
  hoveredId,
  emphasisFilter,
  focusBounds,
  homeSignal,
  sceneBounds,
  scaleRef,
  onHover,
  onLeave,
  onSelect,
  onClear,
  onPickFolder,
}: {
  radarModel: RadarSceneModel;
  selected: LayoutNode | null;
  selectedId: string | null;
  hoveredId: string | null;
  emphasisFilter: EmphasisFilter;
  /** Cinematic fly-to bounds for the shared CameraRig (null = overview/home). */
  focusBounds: Bounds | null;
  /** Monotonic signal that asks the shared CameraRig to return to home. */
  homeSignal: number;
  /** Bounding sphere of the forest; scales the camera's zoom + framing. */
  sceneBounds: Bounds | null;
  scaleRef: { current: number };
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
  onClear: () => void;
  /** Click a folder tag to frame that rail. */
  onPickFolder: (key: string) => void;
}) {
  const { gl } = useThree();
  useEffect(() => {
    gl.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    gl.toneMapping = THREE.ACESFilmicToneMapping;
    gl.toneMappingExposure = 1.05;
  }, [gl]);

  return (
    <>
      <color attach="background" args={[BG]} />
      {/* light fog only: heavy fog was swallowing the entire starfield. */}
      <fogExp2 attach="fog" args={[BG, 0.014]} />

      {/* Lights sculpt only the crystal gem hearts (the cages/nodes are unlit
          emissive); the Environment probe gives each facet its glint. */}
      <ambientLight intensity={0.085} />
      <directionalLight position={[5, 6, 4]} intensity={2.1} color="#fff3e9" />
      <directionalLight position={[-6, -1, -2]} intensity={0.65} color="#bfe2ff" />
      <Environment resolution={128}>
        {/* one shared probe: Claude-tangerine, Codex-cyan, plus warm formers so every
            gem glints in its own hue without the void changing. */}
        <Lightformer form="rect" intensity={1.7} color="#ffcaa0" position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.4} color="#bfeaff" position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="rect" intensity={1.0} color="#ffd9b8" position={[0, -3, -4]} scale={[6, 4, 1]} />
        <Lightformer form="ring" intensity={1.1} color="#ffffff" position={[2, 4, 2]} scale={[2, 2, 1]} />
      </Environment>

      {/* Deep multi-layer star catalog: fine, dense, glacially drifting, and
          deliberately subordinate so the data reads first (see StarCatalog.tsx). */}
      <StarCatalog />

      <CameraRig
        selected={selected}
        focusBounds={focusBounds}
        homeSignal={homeSignal}
        sceneBounds={sceneBounds}
        // The radar board locks the rig (no rotate/pan, straight-on framing).
        locked
      />

      <RadarForest
        model={radarModel}
        selectedId={selectedId}
        hoveredId={hoveredId}
        emphasisFilter={emphasisFilter}
        scaleRef={scaleRef}
        onHover={onHover}
        onLeave={onLeave}
        onSelect={onSelect}
        onClear={onClear}
        onPickFolder={onPickFolder}
      />

      {/* multisampling AA on the composer input stops the thin bright lattice lines
          from sub-pixel shimmering into the bloom pass (the flicker). High smoothing
          plus a higher threshold keep the bloom stable and calm. */}
      <EffectComposer multisampling={4}>
        <Bloom intensity={1.3} luminanceThreshold={0.22} luminanceSmoothing={0.9} mipmapBlur radius={0.85} />
        <Vignette eskil={false} offset={0.22} darkness={0.95} />
      </EffectComposer>
    </>
  );
}

export function WarRoom({ bridge }: { bridge: Bridge }) {
  const [scene, setScene] = useState<SceneState>(() => ({ minimized: false }));
  const [visHidden, setVisHidden] = useState(() => document.hidden);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  // Harness legend filter. null = no filter, so every globe's colour-only dim is 0
  // and the constellation looks unfiltered until a chip lights.
  const [emphasisFilter, setEmphasisFilter] = useState<EmphasisFilter>(null);
  // Radar focus breadcrumb (root -> deep agent ids). Selecting an agent pushes its id;
  // the deepest id drives the CameraRig fly-to (`focusBounds`).
  const [focusStack, setFocusStack] = useState<string[]>([]);
  const [homeSignal, setHomeSignal] = useState(0);
  const [focusBounds, setFocusBounds] = useState<Bounds | null>(null);
  const active = activeFor(scene.summoned, visHidden, scene.minimized);
  // The board never folds (no tab swap), so the forest scale is a constant 1.
  const foldScale = useRef(1);

  useEffect(() => bridge.subscribe(setScene), [bridge]);

  useEffect(() => {
    const onVis = () => setVisHidden(document.hidden);
    document.addEventListener('visibilitychange', onVis);
    return () => {
      document.removeEventListener('visibilitychange', onVis);
    };
  }, []);

  // Radar forest (live agents), empty until the backend emits `radar_state`.
  const radarModel = useMemo<RadarSceneModel>(
    () => scene.radarScene ?? { agents: [], generatedAt: '' },
    [scene.radarScene],
  );
  // Memoised radar layout, also the source of the `id -> {pos, radius}` map that
  // `subtreeBounds` frames against. Computed from the same deterministic layout the
  // forest renders, so the camera frames exactly what is on screen.
  const radarLayout = useMemo(() => layoutRadarScene(radarModel), [radarModel]);
  const radarPositions = useMemo(() => {
    const m = new Map<string, { pos: [number, number, number]; radius: number }>();
    for (const n of radarLayout.nodes) {
      m.set(n.id, { pos: [n.position.x, n.position.y, n.position.z], radius: n.radius });
    }
    return m;
  }, [radarLayout]);
  const selectedNode = useMemo(
    () => radarLayout.nodes.find((n) => n.id === selectedId) ?? null,
    [radarLayout, selectedId],
  );

  // Bounding sphere of the forest, measured from the same laid-out positions the
  // forest renders, feeding the camera so zoom-out and framing SCALE to however large
  // the constellation is.
  const sceneBounds = useMemo<Bounds | null>(
    () =>
      enclosingBounds(
        radarLayout.nodes.map((n) => ({
          pos: [n.position.x, n.position.y, n.position.z] as [number, number, number],
          radius: n.radius,
        })),
      ),
    [radarLayout],
  );

  // Radar detail-panel inputs: the selected live agent and its REAL children (agents
  // whose parentId === the selection). A flat agent yields [], so the panel renders no
  // roster (honest-viz: never a fabricated children list).
  const selectedRadarAgent = useMemo<RadarAgent | null>(
    () => (selectedId ? radarModel.agents.find((a) => a.id === selectedId) ?? null : null),
    [selectedId, radarModel],
  );
  const selectedRadarChildren = useMemo<RadarAgent[]>(
    () =>
      selectedRadarAgent && !isFlatAgent(selectedRadarAgent)
        ? radarModel.agents.filter((a) => a.parentId === selectedRadarAgent.id)
        : [],
    [selectedRadarAgent, radarModel],
  );

  const onHover = useCallback((node: LayoutNode) => setHoveredId(node.id), []);
  const onLeave = useCallback((node: LayoutNode) => setHoveredId((cur) => (cur === node.id ? null : cur)), []);
  const onSelect = useCallback((node: LayoutNode) => {
    // Toggle: clicking the already-focused globe backs out, so there is always an easy
    // way to deselect (alongside empty-click and Esc).
    setSelectedId((cur) => (cur === node.id ? null : node.id));
  }, []);
  const onClear = useCallback(() => setSelectedId(null), []);

  const onDiscoveryHomeDoubleClick = useCallback(
    (event: MouseEvent<HTMLDivElement>) => {
      if (
        !isDiscoveryHomeDoubleClickAllowed({
          selectedId,
          focusDepth: focusStack.length,
          eventTarget: event.target,
        })
      ) {
        return;
      }
      event.preventDefault();
      setHoveredId(null);
      setHomeSignal((signal) => signal + 1);
    },
    [focusStack.length, selectedId],
  );

  const onFilter = useCallback((next: EmphasisFilter) => setEmphasisFilter(next), []);

  // The focus stack is DERIVED from the radar selection so `selectedId` stays the
  // single source of selection truth. Selecting an agent pushes it: a child of the
  // current tip extends the path, an ancestor truncates to it, anything else restarts
  // the path. Clearing the selection empties the stack (camera backs out).
  useEffect(() => {
    if (!selectedId || !radarModel.agents.some((a) => a.id === selectedId)) {
      setFocusStack((cur) => (cur.length ? [] : cur));
      return;
    }
    const id = selectedId;
    const parentId = radarModel.agents.find((a) => a.id === id)?.parentId ?? null;
    setFocusStack((cur) => {
      if (cur[cur.length - 1] === id) return cur; // already the tip
      const at = cur.indexOf(id);
      if (at !== -1) return cur.slice(0, at + 1); // re-selecting an ancestor truncates
      if (parentId !== null && cur[cur.length - 1] === parentId) return [...cur, id]; // dive
      return [id]; // jump elsewhere restarts the path
    });
  }, [selectedId, radarModel]);

  // The camera fly-to bounds. Two sources write it: selecting a bead drives the focus
  // stack, whose deepest crumb frames that agent's subtree; clicking a folder tag
  // frames that whole rail (onPickFolder). Null keeps the CameraRig at the overview.
  useEffect(() => {
    const tip = focusStack[focusStack.length - 1];
    if (!tip || !radarPositions.has(tip)) {
      setFocusBounds((cur) => (cur === null ? cur : null));
      return;
    }
    setFocusBounds(subtreeBounds(radarPositions, radarModel.agents, tip));
  }, [focusStack, radarPositions, radarModel]);

  // Clicking a folder tag frames that rail: the enclosing sphere of every node sharing
  // the rail's y (the root beads plus their subtrees). Reuses the shared CameraRig
  // fly-to via focusBounds, so the glide and back-out match the bead-focus path.
  const onPickFolder = useCallback(
    (key: string) => {
      const cluster = radarLayout.clusters.find((c) => c.key === key);
      if (!cluster) return;
      const members = radarLayout.nodes.filter((n) => Math.abs(n.position.y - cluster.center.y) < 0.01);
      const pts = members.map((n) => ({
        pos: [n.position.x, n.position.y, n.position.z] as [number, number, number],
        radius: n.radius,
      }));
      const bounds = enclosingBounds(pts);
      if (bounds) setFocusBounds(bounds);
    },
    [radarLayout],
  );

  // Breadcrumb controls (lift-only): both work by re-pointing the SELECTION (the single
  // source of truth); the derivation effect above then reconciles the stack, so the
  // camera and detail panel follow with no nested state writes.
  const onClearFocus = useCallback(() => setSelectedId(null), []);
  const onPopFocus = useCallback(
    (index: number) => {
      setSelectedId(index < 0 ? null : focusStack[index] ?? null);
    },
    [focusStack],
  );

  // Radar fit-to-overview: with nothing selected, Escape eases the locked camera back
  // to the framed whole-board overview (bumps homeSignal). Guarded against typing and
  // modifier chords. This bubble-phase listener never fires while a bead is selected:
  // the capture-phase Esc handler below deselects first and stops propagation, so the
  // first Esc backs out the selection and a second Esc (nothing selected) fits.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const el = document.activeElement as HTMLElement | null;
      const typing = !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
      if (typing) return;
      if (e.key === 'Escape') {
        e.preventDefault();
        setHomeSignal((s) => s + 1);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // Esc backs out one level: while a globe is focused, Esc deselects it (swallowed in
  // the capture phase). With nothing selected this listener is inert.
  useEffect(() => {
    if (!selectedId) return;
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key === 'Escape') {
        ev.preventDefault();
        ev.stopImmediatePropagation();
        onClear();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [selectedId, onClear]);

  // Roster jump-to: select a child agent by id, which dives the shared CameraRig onto
  // that globe (selectedId -> selectedNode -> focus) and re-points the panel.
  const onRadarJump = useCallback((id: string) => setSelectedId(id), []);

  // Live radar feed. The backend watcher already pushes `radar_state` on every
  // session-file change (main.ts -> bridge), so the forest is always-on. Two cases the
  // push model cannot cover: a working->idle flip happens when NOTHING changes (the
  // transcript's mtime simply crosses the threshold), and a cold open can predate any
  // change. So while the window is active we also PULL `get_radar_state` right away and
  // on a light interval, keeping liveness honest. `invoke` rejects in the browser
  // sandbox (no Tauri), caught, so the forest is left exactly as it was.
  const fetchRadar = useCallback(async () => {
    try {
      const rs = await invoke('get_radar_state');
      bridge.ingest('radar_scene_ready', rs);
    } catch {
      /* no backend (browser sandbox) or a transient error: never disturb the forest */
    }
  }, [bridge]);

  useEffect(() => {
    if (!active) return;
    fetchRadar(); // immediate on wake
    const id = window.setInterval(fetchRadar, RADAR_VISIBLE_PULL_MS); // fallback; push events remain the fast path
    return () => window.clearInterval(id);
  }, [active, fetchRadar]);

  return (
    <div className="viz-root wd-radar-root" onDoubleClick={onDiscoveryHomeDoubleClick}>
      <Canvas
        dpr={[1, 2]}
        frameloop={frameloopFor(!active)}
        gl={{ antialias: true, alpha: false, powerPreference: 'high-performance' }}
        // Opens far enough back that the constellation frames with room to breathe;
        // |pos| is close to CameraRig's OVERVIEW_DIST so the first frame sits at rest.
        camera={{ position: [4.8, 3.2, 11.7], fov: 46, near: 0.1, far: 140 }}
      >
        <SceneShell
          radarModel={radarModel}
          selected={selectedNode}
          selectedId={selectedId}
          hoveredId={hoveredId}
          emphasisFilter={emphasisFilter}
          focusBounds={focusBounds}
          homeSignal={homeSignal}
          sceneBounds={sceneBounds}
          scaleRef={foldScale}
          onHover={onHover}
          onLeave={onLeave}
          onSelect={onSelect}
          onClear={onClear}
          onPickFolder={onPickFolder}
        />
      </Canvas>

      {/* Slim brand mark, top-left. Orients a first-time viewer and carries the live
          agent count without competing with the board. */}
      <header className="wd-brand" aria-label="WARDEN">
        <span className="wd-brand-name">WARDEN</span>
        <span className="wd-brand-tag">the radar for your coding agents</span>
        <span className="wd-brand-count">{radarModel.agents.length} live</span>
      </header>

      <Breadcrumb
        focusStack={focusStack}
        agents={radarModel.agents.map((a) => ({ id: a.id, label: a.nickname ?? a.label }))}
        onPopFocus={onPopFocus}
        onClearFocus={onClearFocus}
      />

      <FilterBar agents={radarModel.agents} filter={emphasisFilter} onFilter={onFilter} />

      {/* Radar detail panel: its own right-dock. Opens when a globe is selected and the
          camera has dived in; the roster's jump-to flies to a child via onRadarJump. */}
      <div className={`wd-inspector wd-radar-dock ${selectedRadarAgent ? 'is-open' : ''}`}>
        {selectedRadarAgent ? (
          <RadarDetailPanel
            agent={selectedRadarAgent}
            children={selectedRadarChildren}
            onJumpTo={onRadarJump}
            onClose={onClear}
          />
        ) : null}
      </div>

      {/* Honest empty state: the radar is live and watching, there is just nothing
          running yet. Never reads as broken; it says what to do to populate it. */}
      {radarModel.agents.length === 0 ? (
        <div className="wd-radar-empty" aria-live="polite">
          <span className="wd-radar-empty-pulse" aria-hidden />
          <span className="wd-radar-empty-title">Watching for live agents</span>
          <span className="wd-radar-empty-sub">Open Claude Code or Codex and your sessions appear here.</span>
        </div>
      ) : null}
    </div>
  );
}

export default WarRoom;
