// WarRoom.tsx — the persistent orb mind-map AND the whole interface.
//
// This is no longer an ambient backdrop behind a terminal: the terminal is gone
// and the war room is the app. The 3D layer (fresnel orbs, free-orbit camera,
// links, atmosphere) renders the aggregate `OrbSceneModel` built by Rust from
// real detector hits; the DOM `Chrome` layer over it carries the HUD, ask bar,
// live pipeline rail, inspector, legend and empty-state. Hubs are harnesses,
// issue orbs are (harness × pattern), links are issue→own-hub only.
//
// Honest-viz holds throughout: every orb/link/flare maps to a computed signal,
// and off-Fugu runs degrade gracefully (no fabricated counts, verdicts or costs).

import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { Canvas, useThree } from '@react-three/fiber';
import { EffectComposer, Bloom, Vignette } from '@react-three/postprocessing';
import { Environment, Lightformer, Html } from '@react-three/drei';
import * as THREE from 'three';
import { invoke } from '@tauri-apps/api/core';
import type { Bridge, SceneState } from './bridge';
import { harnessTheme } from './harnessTheme';
import { layoutOrbScene } from './orbLayout';
import type { LayoutNode, OrbIssue, OrbLayout, OrbSceneModel } from './orbTypes';
import { Orb } from './Orb';
import { StarCatalog } from './StarCatalog';
import { ConstellationWeb } from './Constellation';
import { CameraRig } from './CameraRig';
import { Chrome, type FixPreview } from './chrome';
import type { RevealFinding } from './compositions/Reveal';
import { NavBar, type ConstellationTab } from './NavBar';
import { RadarForest } from './RadarConstellation';
import { RadarDetailPanel } from './RadarDetailPanel';
import { layoutRadarScene, isFlatAgent } from './radarLayout';
import { TransitionDriver, FoldGroup, makeTransition, beginTransition } from './Transition';
import type { RadarAgent, RadarSceneModel } from './radarTypes';
import { targetDim, type EmphasisFilter } from './emphasis';
import { subtreeBounds, type Bounds } from './cameraFraming';
import IntroVideo from './IntroVideo';

const PlayerHost = lazy(() => import('./PlayerHost'));

const BG = '#020403';

export function frameloopFor(hidden: boolean): 'always' | 'never' {
  return hidden ? 'never' : 'always';
}

// The render loop should run when the daemon summoned the overlay (prod) OR the
// page is genuinely being watched — visible AND focused. `blurred` defaults to
// false so the existing 2-arg call sites keep their old meaning. The focus gate
// is what stops the heavy 60fps war-room from rendering while you sit in your IDE
// during `pnpm tauri dev` (the dev surface is visible but unwatched). A `summoned`
// overlay stays active regardless of focus: the packaged window never takes focus
// ("focus": false) and dismisses on blur on its own, so its render must not hinge
// on it.
export function activeFor(
  summoned: boolean | undefined,
  visHidden: boolean,
  blurred = false,
): boolean {
  return Boolean(summoned) || (!visHidden && !blurred);
}

function humanisePattern(patternId: string): string {
  return (
    patternId
      .split(/[_\s]+/)
      .filter(Boolean)
      .map((w) => w.charAt(0).toUpperCase() + w.slice(1).toLowerCase())
      .join(' ') || 'Unknown Pattern'
  );
}

export function deriveFindings(scene: SceneState): RevealFinding[] {
  return Object.values(scene.verdicts)
    .filter((v) => v.verdict === 'confirmed')
    .sort((a, b) => b.severity - a.severity)
    .map((v) => ({ title: humanisePattern(v.patternId), severity: v.severity, harness: v.harness }));
}

// Live-run fallback model: before `get_orb_scene` lands (or off-Fugu), build a
// provisional scene from the nominated candidates so the map is never empty mid-run.
function fallbackOrbScene(scene: SceneState): OrbSceneModel {
  const agentsByHarness = new Map<string, { count: number; worst: number }>();
  for (const c of scene.candidates) {
    const cur = agentsByHarness.get(c.harness) ?? { count: 0, worst: 0 };
    cur.count += 1;
    cur.worst = Math.max(cur.worst, c.severityHint);
    agentsByHarness.set(c.harness, cur);
  }
  const agents = Array.from(agentsByHarness, ([harness, meta]) => {
    const t = harnessTheme(harness);
    return { id: harness, harness, label: t.label, glyph: t.glyph, color: t.color, sessions: 0, eventCount: 0, totalLoad: meta.count };
  });
  const issues = scene.candidates.map((c) => ({
    id: `${c.harness}:${c.patternId}`,
    agentId: c.harness,
    harness: c.harness,
    patternId: c.patternId,
    title: humanisePattern(c.patternId),
    count: 1,
    severity: c.severityHint,
    rationale: 'Live candidate nominated by the current diagnosis run.',
    estCostTokens: 0,
    estCostMinutes: 0,
    frequency: 0,
    confidence: 0,
    sessionIds: [c.sessionId],
    evidence: [],
  }));
  return {
    agents,
    issues,
    links: issues.map((issue) => ({ source: issue.agentId, target: issue.id, kind: 'agent_issue' as const })),
    guidance: { doItems: [], stopItems: [] },
  };
}

// Harness name under each hub — the explicit "this is Claude / this is Codex".
function HubLabels({ hubs }: { hubs: LayoutNode[] }) {
  return (
    <>
      {hubs.map((h) => {
        const t = harnessTheme(h.harness);
        const r = h.territoryRadius ?? 2;
        return (
          <Html
            key={`label-${h.id}`}
            position={[h.position.x, h.position.y - r * 0.84, h.position.z]}
            center
            zIndexRange={[6, 0]}
            style={{ pointerEvents: 'none' }}
          >
            <div className="wd-hub-label" style={{ '--harness': t.color } as CSSProperties}>
              <span className="wd-hub-label-glyph">{t.glyph}</span>
              {t.label}
            </div>
          </Html>
        );
      })}
    </>
  );
}

// The Habits DATA forest only (orbs + tethers + hub labels), wrapped in the fold
// group. Carries no background/lights/camera/post — those live once in `SceneShell`
// so the void persists across the Habits↔Radar swap (mirrors radar's RadarForest).
function HabitsForest({
  layout,
  selectedId,
  hoveredId,
  emphasisFilter,
  scaleRef,
  onHover,
  onLeave,
  onSelect,
  onClear,
}: {
  layout: OrbLayout;
  selectedId: string | null;
  hoveredId: string | null;
  /** Active legend filter; each globe's colour-only `dimTarget` is derived from it. */
  emphasisFilter: EmphasisFilter;
  /** Live fold scale for the constellation swap (1 = at rest). */
  scaleRef: { current: number };
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
  onClear: () => void;
}) {
  const selectedAgent = useMemo(
    () => layout.nodes.find((n) => n.id === selectedId)?.agentId,
    [layout, selectedId],
  );
  const hubs = useMemo(() => layout.nodes.filter((n) => n.kind === 'hub'), [layout]);

  return (
    <FoldGroup scaleRef={scaleRef}>
      <group onPointerMissed={onClear}>
        <ConstellationWeb layout={layout} />
        {layout.nodes.map((node, i) => (
          <Orb
            key={node.id}
            node={node}
            selected={selectedId === node.id}
            hovered={hoveredId === node.id}
            dimmed={Boolean(selectedId && selectedId !== node.id && node.agentId !== selectedAgent)}
            // Legend filter → colour-only dim. Severity buckets read the issue's
            // severity; harness filters read the node's harness (both tabs). With a
            // null filter `targetDim` is 0, so the look is unchanged until Task 10
            // lights a chip. Reuses the pure `emphasis` module (no logic forked here).
            dimTarget={targetDim({ harness: node.harness, severity: node.issue?.severity }, emphasisFilter)}
            appearDelay={Math.min(i * 0.045, 0.6)}
            onHover={onHover}
            onLeave={onLeave}
            onSelect={onSelect}
          />
        ))}
      </group>
      <HubLabels hubs={hubs} />
    </FoldGroup>
  );
}

// The persistent scene shell — the SINGLE always-mounted void (background, fog,
// lights, Environment, starfield, the shared free-orbit CameraRig and the post
// stack). It is rendered UNCONDITIONALLY by WarRoom, so a Habits↔Radar tab swap
// only ever changes the ONE forest child slot (already folded to nothing at the
// swap); none of the void unmounts, so the whole app is one continuous motion with
// no flicker. The Environment carries formers for every harness hue (Claude-emerald,
// Codex-violet, warm) so neither tab loses its glint.
function SceneShell({
  displayTab,
  habitsLayout,
  radarModel,
  selected,
  selectedId,
  hoveredId,
  emphasisFilter,
  focusBounds,
  scaleRef,
  onHover,
  onLeave,
  onSelect,
  onClear,
}: {
  displayTab: ConstellationTab;
  habitsLayout: OrbLayout;
  radarModel: RadarSceneModel;
  selected: LayoutNode | null;
  selectedId: string | null;
  hoveredId: string | null;
  /** Active legend filter, forwarded to whichever forest is on screen. */
  emphasisFilter: EmphasisFilter;
  /** Cinematic fly-to bounds for the shared CameraRig (null = overview/home). */
  focusBounds: Bounds | null;
  scaleRef: { current: number };
  onHover: (node: LayoutNode) => void;
  onLeave: (node: LayoutNode) => void;
  onSelect: (node: LayoutNode) => void;
  onClear: () => void;
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
      {/* light fog only — heavy fog was swallowing the entire starfield. */}
      <fogExp2 attach="fog" args={[BG, 0.014]} />

      {/* Lights sculpt only the crystal gem hearts (the cages/nodes are unlit
          emissive); the Environment probe gives each facet its glint. */}
      <ambientLight intensity={0.1} />
      <directionalLight position={[5, 6, 4]} intensity={2.2} color="#e6fff0" />
      <directionalLight position={[-6, -1, -2]} intensity={0.7} color="#9fd0ff" />
      <Environment resolution={128}>
        {/* one shared probe for both tabs — emerald, violet + warm formers so
            Claude, Codex and the radar gems all glint without the void changing. */}
        <Lightformer form="rect" intensity={1.7} color="#bfffe0" position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.3} color="#cab8ff" position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="rect" intensity={1.0} color="#ffd9b8" position={[0, -3, -4]} scale={[6, 4, 1]} />
        <Lightformer form="ring" intensity={1.1} color="#ffffff" position={[2, 4, 2]} scale={[2, 2, 1]} />
      </Environment>

      {/* Deep multi-layer star catalog — fine, dense, glacially drifting, and
          deliberately subordinate so the data reads first (see StarCatalog.tsx). */}
      <StarCatalog />

      <CameraRig selected={selected} focusBounds={focusBounds} />

      {/* The ONLY thing that swaps on a tab change — folded to nothing at the swap
          midpoint, then bloomed back. The shell around it never remounts. */}
      {displayTab === 'radar' ? (
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
        />
      ) : (
        <HabitsForest
          layout={habitsLayout}
          selectedId={selectedId}
          hoveredId={hoveredId}
          emphasisFilter={emphasisFilter}
          scaleRef={scaleRef}
          onHover={onHover}
          onLeave={onLeave}
          onSelect={onSelect}
          onClear={onClear}
        />
      )}

      {/* multisampling AA on the composer input stops the thin bright lattice
          lines from sub-pixel shimmering into the bloom pass (the flicker). High
          smoothing + a higher threshold keep the bloom stable + calm. */}
      <EffectComposer multisampling={4}>
        <Bloom intensity={0.93} luminanceThreshold={0.27} luminanceSmoothing={0.95} mipmapBlur radius={0.74} />
        <Vignette eskil={false} offset={0.2} darkness={0.92} />
      </EffectComposer>
    </>
  );
}

export function WarRoom({ bridge, forceIntro }: { bridge: Bridge; forceIntro?: boolean }) {
  const [scene, setScene] = useState<SceneState>(() => ({
    phase: 'idle',
    candidates: [],
    verdicts: {},
    pulses: [],
    usage: {},
    clustered: 0,
  }));
  const [visHidden, setVisHidden] = useState(() => document.hidden);
  // Twin of `visHidden`: true while the window is blurred (e.g. you alt-tabbed to
  // your editor during `pnpm tauri dev`). Combined with `visHidden` it lets the
  // Canvas pause the heavy render whenever nobody is actually watching the page.
  const [blurred, setBlurred] = useState(() =>
    typeof document.hasFocus === 'function' ? !document.hasFocus() : false,
  );
  // `tab` is the nav INTENT (the lit tab — flips instantly on click); `displayTab`
  // is the constellation actually on screen, which only swaps at the PEAK of the
  // hyperspace jump so the change happens hidden under the streaks.
  const [tab, setTab] = useState<ConstellationTab>('habits');
  const [displayTab, setDisplayTab] = useState<ConstellationTab>('habits');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  // Interactive-legend filter (Task 10 lights the chips). null = no filter, so every
  // globe's colour-only `dimTarget` is 0 and the constellation looks exactly as it
  // does today. Severity buckets apply to Habits issue orbs; harness filters apply
  // to both tabs. Lifted here so one source of truth drives both forests.
  const [emphasisFilter, setEmphasisFilter] = useState<EmphasisFilter>(null);
  // Radar focus breadcrumb (root→deep agent ids). Selecting a radar agent pushes its
  // id; the deepest id drives the CameraRig fly-to (`focusBounds`). Task 10 builds the
  // breadcrumb UI; here we only own the stack + expose pop/clear.
  const [focusStack, setFocusStack] = useState<string[]>([]);
  const [fixPreview, setFixPreview] = useState<FixPreview | undefined>();
  const [loadingFix, setLoadingFix] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);
  const active = activeFor(scene.summoned, visHidden, blurred);
  const introPlayed = useRef(!document.hidden);
  const [showIntro, setShowIntro] = useState(false);

  // ── tab fold transition (Radar spec §8 — nothing ever cuts) ─────────────────
  // The single warm <Canvas> never remounts. On a Habits↔Radar switch the current
  // constellation folds down to nothing, the content swaps at zero size, then the
  // next blooms back out (Transition.tsx). `transitionRef` is the shared fold state
  // the in-Canvas driver runs down each frame; `foldScale` is the live scale the
  // constellation FoldGroups read; `pendingTab` is where we're headed (committed to
  // the screen at the fold midpoint).
  const transitionRef = useRef(makeTransition());
  const foldScale = useRef(1);
  const pendingTab = useRef<ConstellationTab>('habits');

  useEffect(() => bridge.subscribe(setScene), [bridge]);

  useEffect(() => {
    if (forceIntro) setShowIntro(true);
  }, [forceIntro]);

  useEffect(() => {
    if (active && !introPlayed.current) {
      introPlayed.current = true;
      setShowIntro(true);
    }
  }, [active]);

  useEffect(() => {
    const onVis = () => setVisHidden(document.hidden);
    const onFocus = () => setBlurred(false);
    const onBlur = () => setBlurred(true);
    document.addEventListener('visibilitychange', onVis);
    window.addEventListener('focus', onFocus);
    window.addEventListener('blur', onBlur);
    return () => {
      document.removeEventListener('visibilitychange', onVis);
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('blur', onBlur);
    };
  }, []);

  const model = useMemo(() => scene.orbScene ?? fallbackOrbScene(scene), [scene.orbScene, scene.candidates]);
  const layout = useMemo(() => layoutOrbScene(model), [model]);
  // Radar forest (live agents) — empty until the backend emits `radar_state`.
  const radarModel = useMemo<RadarSceneModel>(() => scene.radarScene ?? { agents: [], generatedAt: '' }, [scene.radarScene]);
  // Memoised radar layout — also the source of the `id → {pos, radius}` map that
  // `subtreeBounds` frames against. Computed from the same deterministic layout the
  // forest renders, so the camera frames exactly what's on screen.
  const radarLayout = useMemo(() => layoutRadarScene(radarModel), [radarModel]);
  const radarPositions = useMemo(() => {
    const m = new Map<string, { pos: [number, number, number]; radius: number }>();
    for (const n of radarLayout.nodes) {
      m.set(n.id, { pos: [n.position.x, n.position.y, n.position.z], radius: n.radius });
    }
    return m;
  }, [radarLayout]);
  const activeLayout = displayTab === 'radar' ? radarLayout : layout;
  const selectedNode = useMemo(() => activeLayout.nodes.find((n) => n.id === selectedId) ?? null, [activeLayout, selectedId]);
  const hoveredNode = useMemo(() => activeLayout.nodes.find((n) => n.id === hoveredId) ?? null, [activeLayout, hoveredId]);

  // Radar detail-panel inputs: the selected live agent and its REAL children
  // (agents whose parentId === the selection). A flat agent yields []; the panel
  // then renders no roster (honest-viz — never a fabricated children list).
  const selectedRadarAgent = useMemo<RadarAgent | null>(
    () => (displayTab === 'radar' && selectedId ? radarModel.agents.find((a) => a.id === selectedId) ?? null : null),
    [displayTab, selectedId, radarModel],
  );
  // A flat agent (VS Code Codex / unknown harness) yields [] even if a drifted
  // payload pointed a stray child at it — the roster mirrors the layout's flat-globe
  // guard so the panel never fabricates a child the constellation refused to orbit.
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
    // Toggle: clicking the already-focused orb backs out, so when a globe fills the
    // screen there's always an easy way to deselect (alongside empty-click + Esc).
    setSelectedId((cur) => (cur === node.id ? null : node.id));
    setFixPreview(undefined);
  }, []);
  const onClear = useCallback(() => {
    setSelectedId(null);
    setFixPreview(undefined);
  }, []);

  // ── interactive legend ───────────────────────────────────────────────────────
  // Lift-only: set the active filter. Task 10's legend chips call this; passing the
  // same filter again (a chip toggled off) is the caller's job — we just store it.
  const onFilter = useCallback((next: EmphasisFilter) => setEmphasisFilter(next), []);

  // ── radar focus breadcrumb ───────────────────────────────────────────────────
  // The stack is DERIVED from the radar selection so `selectedId` stays the single
  // source of selection truth (hover/cross-fade untouched). Selecting an agent pushes
  // it: a child of the current tip extends the path, an ancestor truncates to it, and
  // anything else restarts the path at that agent. Leaving the radar tab or clearing
  // the selection empties the stack (camera backs out). Deterministic — built purely
  // from the previous stack + the new selection + the live parent links.
  useEffect(() => {
    if (displayTab !== 'radar') {
      setFocusStack((cur) => (cur.length ? [] : cur));
      return;
    }
    if (!selectedId || !radarModel.agents.some((a) => a.id === selectedId)) {
      setFocusStack((cur) => (cur.length ? [] : cur));
      return;
    }
    const id = selectedId;
    const parentId = radarModel.agents.find((a) => a.id === id)?.parentId ?? null;
    setFocusStack((cur) => {
      if (cur[cur.length - 1] === id) return cur; // already the tip
      const at = cur.indexOf(id);
      if (at !== -1) return cur.slice(0, at + 1); // re-selecting an ancestor → truncate
      if (parentId !== null && cur[cur.length - 1] === parentId) return [...cur, id]; // dive
      return [id]; // jump elsewhere → restart the path
    });
  }, [displayTab, selectedId, radarModel]);

  // The deepest crumb frames the camera: its subtree bounding sphere (the agent + all
  // live descendants) drives the CameraRig fly-to. Empty stack → null → overview pose.
  const focusBounds = useMemo<Bounds | null>(() => {
    const tip = focusStack[focusStack.length - 1];
    if (!tip || !radarPositions.has(tip)) return null;
    return subtreeBounds(radarPositions, radarModel.agents, tip);
  }, [focusStack, radarPositions, radarModel]);

  // Breadcrumb controls for Task 10 (lift-only; no UI built here). Both work by
  // re-pointing the SELECTION (the single source of truth); the derivation effect
  // above then reconciles the stack — selecting an ancestor truncates it, clearing
  // empties it — so the camera + detail panel follow with no nested state writes.
  const onClearFocus = useCallback(() => {
    setSelectedId(null);
    setFixPreview(undefined);
  }, []);
  const onPopFocus = useCallback(
    (index: number) => {
      setSelectedId(index < 0 ? null : focusStack[index] ?? null);
      setFixPreview(undefined);
    },
    [focusStack],
  );

  // Esc backs out one level: while an orb is focused the first Esc deselects, and
  // is swallowed (capture phase) before main.ts's global handler so the overlay
  // stays open. With nothing selected this listener is inert and Esc falls through
  // to the daemon dismiss exactly as before.
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

  // Roster jump-to: select a child agent by id, which dives the shared CameraRig
  // onto that globe (selectedId → selectedNode → focus) and re-points the panel.
  const onRadarJump = useCallback((id: string) => setSelectedId(id), []);

  // ── live radar feed ─────────────────────────────────────────────────────────
  // The backend watcher already pushes `radar_state` on every session-file change
  // (main.ts → bridge), so the forest is always-on. But two cases the push model
  // can't cover: a working→idle flip happens when NOTHING changes (the transcript's
  // mtime simply crosses the threshold), and a cold open can predate any change. So
  // while the radar is actually on screen we also PULL `get_radar_state` right away
  // and on a light interval, keeping liveness honest. `invoke` rejects in the browser
  // QA harness (no Tauri) → caught → the forest is left exactly as it was.
  const fetchRadar = useCallback(async () => {
    try {
      const rs = await invoke('get_radar_state');
      bridge.ingest('radar_scene_ready', rs);
    } catch {
      /* no backend (harness) or a transient error — never disturb the live forest */
    }
  }, [bridge]);

  useEffect(() => {
    if (displayTab !== 'radar' || !active) return;
    fetchRadar(); // immediate on entering the radar / on summon
    const id = window.setInterval(fetchRadar, 3000); // keep idle/working state current
    return () => window.clearInterval(id);
  }, [displayTab, active, fetchRadar]);

  // The constellation on screen swaps at the FOLD MIDPOINT (folded to nothing), so
  // the change is hidden at zero size — never a visible cut.
  const onFoldMidpoint = useCallback(() => setDisplayTab(pendingTab.current), []);
  // Fold landed — the driver has already cleared `transitionRef.active` and reset the
  // scale to 1; selection/hover were reset at launch. Kept as a seam for arrival polish.
  const onFoldDone = useCallback(() => {}, []);

  // Switching constellations clears the per-scene hover/selection (the two scenes
  // have disjoint node-id spaces) so no inspector points at a stale node, lights the
  // target tab instantly, and starts the fold. A tab click mid-fold is ignored so the
  // collapse→bloom always completes cleanly.
  const onTab = useCallback((next: ConstellationTab) => {
    if (transitionRef.current.active) return;
    if (next === tab) return;
    setSelectedId(null);
    setHoveredId(null);
    setFixPreview(undefined);
    pendingTab.current = next;
    setTab(next);
    beginTransition(transitionRef);
  }, [tab]);

  const onAsk = useCallback(
    async (query: string) => {
      if (scene.running) return;
      setRunError(null);
      bridge.ingest('diagnosis_run', { running: true, query });
      try {
        const d = await invoke('run_diagnosis', {
          scope: { harness: 'claude_code', query, force: false, max_files: null },
        });
        bridge.ingest('diagnosis_loaded', d);
        bridge.ingest('diagnosis_run', { running: false });
      } catch (e) {
        setRunError(String(e));
        bridge.ingest('diagnosis_run_failed', {});
      }
    },
    [bridge, scene.running],
  );

  const onRequestFix = useCallback(async (issue: OrbIssue) => {
    setLoadingFix(true);
    setFixPreview(undefined);
    try {
      const preview = issue.findingId
        ? await invoke<FixPreview>('get_fix_preview', { findingId: issue.findingId })
        : await invoke<FixPreview>('get_orb_fix_preview', { issueId: issue.id });
      setFixPreview(preview);
    } catch {
      setFixPreview({
        finding_id: issue.findingId ?? issue.id,
        pattern_id: issue.patternId,
        target_path: 'WARDEN overlay',
        diff: 'Fix preview is available in the WARDEN app. This browser QA stage never writes or applies fixes.',
        applied: false,
      });
    } finally {
      setLoadingFix(false);
    }
  }, []);

  const onDismiss = useCallback(() => {
    invoke('hide_overlay').catch(() => {});
  }, []);

  const findings = useMemo(() => deriveFindings(scene), [scene.verdicts]);
  const diagnosisId = scene.diagnosisId ?? 'diagnosis';

  return (
    <div className={`viz-root viz-phase-${scene.phase} viz-orb-map`}>
      <Canvas
        dpr={[1, 2]}
        frameloop={frameloopFor(!active)}
        gl={{ antialias: true, alpha: false, powerPreference: 'high-performance' }}
        // Opens further back than before so the (now wider-spaced) constellation
        // frames with room to breathe; |pos| ≈ CameraRig's OVERVIEW_DIST so the
        // first frame already sits at rest. Both tabs share this one camera.
        camera={{ position: [4.8, 3.2, 11.7], fov: 46, near: 0.1, far: 140 }}
      >
        {/* The fold driver — a sibling of the swapped scene so it survives the
            mid-fold content swap and keeps running the collapse→bloom throughout. */}
        <TransitionDriver
          stateRef={transitionRef}
          scaleRef={foldScale}
          onMidpoint={onFoldMidpoint}
          onDone={onFoldDone}
        />

        {/* One persistent shell; only its inner forest swaps on a tab change, so the
            void never remounts — the whole app stays one continuous animation. */}
        <SceneShell
          displayTab={displayTab}
          habitsLayout={layout}
          radarModel={radarModel}
          selected={selectedNode}
          selectedId={selectedId}
          hoveredId={hoveredId}
          emphasisFilter={emphasisFilter}
          focusBounds={focusBounds}
          scaleRef={foldScale}
          onHover={onHover}
          onLeave={onLeave}
          onSelect={onSelect}
          onClear={onClear}
        />
      </Canvas>

      <NavBar
        tab={tab}
        onTab={onTab}
        counts={{ habits: layout.nodes.filter((n) => n.kind === 'issue').length, radar: radarModel.agents.length }}
      />

      {/* Chrome is the Habits inspector (keys off node.issue/agent). On the radar
          tab the live selection flows to RadarSceneBody via selectedId; the radar
          detail panel is Phase 3, so keep the Habits inspector closed here rather
          than feeding it a radar node it cannot render. */}
      <Chrome
        scene={scene}
        model={model}
        tab={tab}
        hoveredNode={displayTab === 'radar' ? null : hoveredNode}
        selectedNode={displayTab === 'radar' ? null : selectedNode}
        emphasisFilter={emphasisFilter}
        focusStack={focusStack}
        running={Boolean(scene.running)}
        error={runError}
        fixPreview={fixPreview}
        loadingFix={loadingFix}
        onAsk={onAsk}
        onRequestFix={onRequestFix}
        onClearSelection={onClear}
        onDismiss={onDismiss}
        onFilter={onFilter}
        onPopFocus={onPopFocus}
        onClearFocus={onClearFocus}
      />

      {/* Radar detail panel — its own right-dock (the Chrome inspector is Habits-
          only). Opens when a radar globe is selected and the camera has dived in;
          the roster's jump-to flies to a child via onRadarJump (select + focus). */}
      <div className={`wd-inspector wd-radar-dock ${displayTab === 'radar' && selectedRadarAgent ? 'is-open' : ''}`}>
        {selectedRadarAgent ? (
          <RadarDetailPanel
            agent={selectedRadarAgent}
            children={selectedRadarChildren}
            onJumpTo={onRadarJump}
            onClose={onClear}
          />
        ) : null}
      </div>

      {/* Honest empty state — the radar is live and watching, there's just nothing
          running yet. Never reads as "broken": it says what to do to populate it. */}
      {displayTab === 'radar' && radarModel.agents.length === 0 ? (
        <div className="wd-radar-empty" aria-live="polite">
          <span className="wd-radar-empty-pulse" aria-hidden />
          <span className="wd-radar-empty-title">Watching for live agents</span>
          <span className="wd-radar-empty-sub">Open Claude Code or Codex and your sessions appear here.</span>
        </div>
      ) : null}

      {showIntro && <IntroVideo onEnded={() => setShowIntro(false)} />}
      {scene.phase === 'reveal' && (
        <Suspense fallback={null}>
          <PlayerHost kind="reveal" findings={findings} diagnosisId={diagnosisId} />
        </Suspense>
      )}
    </div>
  );
}

export default WarRoom;
