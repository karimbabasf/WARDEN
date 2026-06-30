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

import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties, type MouseEvent } from 'react';
import { Canvas, useThree } from '@react-three/fiber';
import { EffectComposer, Bloom, Vignette } from '@react-three/postprocessing';
import { Environment, Lightformer, Html } from '@react-three/drei';
import * as THREE from 'three';
import { invoke } from '@tauri-apps/api/core';
import type { Bridge, SceneState } from '@/viz/shared/state/bridge';
import { harnessTheme } from '@/viz/shared/theme/harnessTheme';
import { layoutOrbScene } from '@/viz/modules/habits/orbLayout';
import type { LayoutNode, OrbIssue, OrbLayout, OrbSceneModel } from '@/viz/shared/types/orbTypes';
import { Orb } from '@/viz/modules/habits/Orb';
import { StarCatalog } from '@/viz/shared/scene/StarCatalog';
import { ConstellationWeb } from '@/viz/modules/habits/Constellation';
import { CameraRig } from '@/viz/shared/scene/CameraRig';
import { Chrome, type Artifact, type FixPreview } from './chrome';
import type { RevealFinding } from '@/viz/modules/cinematics/compositions/Reveal';
import { NavBar, PRIMARY_CONSTELLATION_TAB, type ConstellationTab } from './NavBar';
import { RadarForest } from '@/viz/modules/radar/RadarConstellation';
import { RadarDetailPanel } from '@/viz/modules/radar/RadarDetailPanel';
import { FilterBar } from './FilterBar';
import { Sidebar } from './Sidebar';
import { buildRadarRoster, buildHabitsRoster } from '@/viz/modules/radar/rosterTree';
import { layoutRadarScene, isFlatAgent } from '@/viz/modules/radar/radarLayout';
import { radarHarness } from '@/viz/modules/radar/radarTheme';
import { TransitionDriver, FoldGroup, makeTransition, beginTransition } from '@/viz/shared/scene/Transition';
import type { RadarAgent, RadarSceneModel } from '@/viz/shared/types/radarTypes';
import { targetDim, matchesFilter, type EmphasisFilter } from '@/viz/shared/lib/emphasis';
import { subtreeBounds, enclosingBounds, type Bounds } from '@/viz/shared/scene/cameraFraming';
import { frameloopFor } from '@/viz/shared/scene/frameloop';
import IntroVideo from '@/viz/modules/cinematics/IntroVideo';

const PlayerHost = lazy(() => import('@/viz/modules/cinematics/PlayerHost'));

const BG = '#020403';

export const RADAR_VISIBLE_PULL_MS = 750;

// The render loop runs whenever the window is on screen — even unfocused or sitting
// on another display. The ONLY thing that pauses it is MINIMIZE (CPU saver). A
// summoned overlay is active regardless of the page-visibility flag (a native
// .show() may leave document.hidden stale-true). Dev/browser (no summon) keys off
// page visibility so a hidden tab still pauses.
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

// Upsert one artifact into the history list by id (newest write wins), keeping
// it newest-first. Pure so the ledger-merge is unit-testable without a render.
// A re-staged/re-applied artifact replaces its prior row rather than duplicating.
export function mergeArtifact(prev: Artifact[], next: Artifact): Artifact[] {
  const without = prev.filter((a) => a.id !== next.id);
  return [next, ...without];
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

export function chromeModelForTab(
  tab: ConstellationTab,
  habitsModel: OrbSceneModel,
  radarModel: RadarSceneModel,
): OrbSceneModel {
  if (tab !== 'radar') return habitsModel;
  return {
    ...habitsModel,
    agents: radarModel.agents.map((agent) => {
      const t = radarHarness(agent.harness);
      return {
        id: agent.id,
        harness: agent.harness,
        label: agent.nickname ?? agent.label,
        glyph: t.glyph,
        color: t.color,
        sessions: 1,
        eventCount: agent.recentActivity.length,
        totalLoad: agent.contextTokens,
      };
    }),
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
            // …and a matching orb POPS (extra glow + a touch of scale) so the chosen
            // severity/harness stands out, not just the others dimming.
            emphasized={emphasisFilter !== null && matchesFilter({ harness: node.harness, severity: node.issue?.severity }, emphasisFilter)}
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
  homeSignal,
  sceneBounds,
  flyMode,
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
  /** Monotonic signal that asks the shared CameraRig to return to home. */
  homeSignal: number;
  /** Bounding sphere of the active forest; scales the camera's zoom + framing. */
  sceneBounds: Bounds | null;
  /** When true, the shared CameraRig switches to free-fly (WASD) navigation. */
  flyMode: boolean;
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
      <ambientLight intensity={0.085} />
      <directionalLight position={[5, 6, 4]} intensity={2.1} color="#fff3e9" />
      <directionalLight position={[-6, -1, -2]} intensity={0.65} color="#bfe2ff" />
      <Environment resolution={128}>
        {/* one shared probe for both tabs — Claude-tangerine, Codex-cyan + warm
            formers so every gem glints in its own hue without the void changing. */}
        <Lightformer form="rect" intensity={1.7} color="#ffcaa0" position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.4} color="#bfeaff" position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="rect" intensity={1.0} color="#ffd9b8" position={[0, -3, -4]} scale={[6, 4, 1]} />
        <Lightformer form="ring" intensity={1.1} color="#ffffff" position={[2, 4, 2]} scale={[2, 2, 1]} />
      </Environment>

      {/* Deep multi-layer star catalog — fine, dense, glacially drifting, and
          deliberately subordinate so the data reads first (see StarCatalog.tsx). */}
      <StarCatalog />

      <CameraRig
        selected={selected}
        focusBounds={focusBounds}
        homeSignal={homeSignal}
        sceneBounds={sceneBounds}
        flyMode={flyMode}
      />

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
        <Bloom intensity={1.3} luminanceThreshold={0.22} luminanceSmoothing={0.9} mipmapBlur radius={0.85} />
        <Vignette eskil={false} offset={0.22} darkness={0.95} />
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
  // `tab` is the nav INTENT (the lit tab — flips instantly on click); `displayTab`
  // is the constellation actually on screen, which only swaps at the PEAK of the
  // hyperspace jump so the change happens hidden under the streaks.
  const [tab, setTab] = useState<ConstellationTab>(PRIMARY_CONSTELLATION_TAB);
  const [displayTab, setDisplayTab] = useState<ConstellationTab>(PRIMARY_CONSTELLATION_TAB);
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
  const [homeSignal, setHomeSignal] = useState(0);
  const [fixPreview, setFixPreview] = useState<FixPreview | undefined>();
  const [loadingFix, setLoadingFix] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);
  // ── M4 Forge: the reversible apply/revert write loop ──────────────────────
  // `artifact` is the write record for the currently-open finding (drives the
  // applied badge + Revert affordance); `artifacts` is the full guardrail ledger
  // (applied + reverted history). `applying`/`reverting` gate the buttons while
  // an invoke is in flight. All four mirror the REAL backend Artifact rows —
  // never fabricated client state.
  const [artifact, setArtifact] = useState<Artifact | undefined>();
  const [artifacts, setArtifacts] = useState<Artifact[]>([]);
  const [applying, setApplying] = useState(false);
  const [reverting, setReverting] = useState(false);
  const [ledgerOpen, setLedgerOpen] = useState(false);
  const active = activeFor(scene.summoned, visHidden, scene.minimized);
  const introPlayed = useRef(!document.hidden);
  const [showIntro, setShowIntro] = useState(false);
  // Roster sidebar (left dock) — closed by default; the ≡ button and the panel's
  // ✕ both toggle it. Session-local (not persisted); a tab swap keeps it as-is.
  const [sidebarOpen, setSidebarOpen] = useState(false);

  // ── tab fold transition (Radar spec §8 — nothing ever cuts) ─────────────────
  // The single warm <Canvas> never remounts. On a Habits↔Radar switch the current
  // constellation folds down to nothing, the content swaps at zero size, then the
  // next blooms back out (Transition.tsx). `transitionRef` is the shared fold state
  // the in-Canvas driver runs down each frame; `foldScale` is the live scale the
  // constellation FoldGroups read; `pendingTab` is where we're headed (committed to
  // the screen at the fold midpoint).
  const transitionRef = useRef(makeTransition());
  const foldScale = useRef(1);
  const pendingTab = useRef<ConstellationTab>(PRIMARY_CONSTELLATION_TAB);

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
    document.addEventListener('visibilitychange', onVis);
    return () => {
      document.removeEventListener('visibilitychange', onVis);
    };
  }, []);

  const model = useMemo(() => scene.orbScene ?? fallbackOrbScene(scene), [scene.orbScene, scene.candidates]);
  const layout = useMemo(() => layoutOrbScene(model), [model]);
  // Radar forest (live agents) — empty until the backend emits `radar_state`.
  const radarModel = useMemo<RadarSceneModel>(() => scene.radarScene ?? { agents: [], generatedAt: '' }, [scene.radarScene]);
  const chromeModel = useMemo(() => chromeModelForTab(tab, model, radarModel), [tab, model, radarModel]);
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

  // Bounding sphere of the active forest, measured from the same laid-out positions
  // the forest renders — feeds the camera so zoom-out + framing SCALE to however
  // large the constellation is (fixes "can't zoom out / can't reach far agents"
  // once the forest grows past the old fixed cage).
  const sceneBounds = useMemo<Bounds | null>(
    () =>
      enclosingBounds(
        activeLayout.nodes.map((n) => ({
          pos: [n.position.x, n.position.y, n.position.z] as [number, number, number],
          radius: n.radius,
        })),
      ),
    [activeLayout],
  );

  // Free-fly toggle: `F` flips the shared camera between uncaged orbit and a 6DOF
  // fly camera; `Esc` exits fly. Guarded so it never fires while typing in the ask
  // bar (or any input / contenteditable).
  const [flyMode, setFlyMode] = useState(false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const el = document.activeElement as HTMLElement | null;
      const typing =
        !!el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
      if (typing) return;
      if (e.key === 'f' || e.key === 'F') {
        e.preventDefault();
        setFlyMode((v) => !v);
      } else if (e.key === 'Escape') {
        setFlyMode((v) => (v ? false : v));
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  // Roster (left sidebar) content for the constellation on screen: radar agents
  // grouped by harness with subagents nested, or the habit orbs grouped by harness.
  // Built from the SAME models the forest renders (honest-viz) so a row click
  // selects exactly that globe via the shared `selectedId`.
  const rosterGroups = useMemo(
    () => (displayTab === 'radar' ? buildRadarRoster(radarModel.agents) : buildHabitsRoster(layout)),
    [displayTab, radarModel, layout],
  );
  const rosterHeader = useMemo(() => {
    if (displayTab === 'radar') {
      const n = radarModel.agents.length;
      const working = radarModel.agents.filter((a) => a.status === 'working').length;
      return `${n} ${n === 1 ? 'agent' : 'agents'} · ${working} working`;
    }
    const habits = layout.nodes.filter((node) => node.kind === 'issue').length;
    return `${habits} ${habits === 1 ? 'habit' : 'habits'}`;
  }, [displayTab, radarModel, layout]);

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
    setArtifact(undefined); // the open write record is per-finding — reset on a new dive
  }, []);
  const onClear = useCallback(() => {
    setSelectedId(null);
    setFixPreview(undefined);
    setArtifact(undefined);
  }, []);

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

  // ── interactive legend ───────────────────────────────────────────────────────
  // Lift-only: set the active filter. Task 10's legend chips call this; passing the
  // same filter again (a chip toggled off) is the caller's job — we just store it.
  const onFilter = useCallback((next: EmphasisFilter) => setEmphasisFilter(next), []);

  // Sidebar toggle, and the roster row → select that globe. Picking reuses the
  // single selection source so the existing camera dive (focusStack/CameraRig) +
  // detail dock follow for free on both tabs (a radar agent id and a habits node
  // id both address `selectedId`).
  const onToggleSidebar = useCallback(() => setSidebarOpen((o) => !o), []);
  const onPickRoster = useCallback((id: string) => setSelectedId(id), []);

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

  // Esc backs out one level: while an orb is focused, Esc deselects it (swallowed in
  // the capture phase). With nothing selected this listener is inert and Esc does
  // nothing — the overlay stays on screen. Dismissal is explicit only: the
  // Minimize / Close window controls, the tray, or the ⌘⌥⌃M hotkey.
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
    const id = window.setInterval(fetchRadar, RADAR_VISIBLE_PULL_MS); // fallback; push events remain the fast path
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

  // ── M4 Forge: stage → apply → revert, wired to the FROZEN CONTRACT ─────────
  // Refresh the full guardrail ledger from the backend (the single source of
  // truth for history). `invoke` rejects in the browser-QA harness (no Tauri) →
  // caught → the ledger is left as-is, never a fabricated row.
  const refreshLedger = useCallback(async () => {
    try {
      const all = await invoke<Artifact[]>('list_artifacts', {});
      setArtifacts(Array.isArray(all) ? all : []);
    } catch {
      /* no backend (harness) — keep whatever we have */
    }
  }, []);

  useEffect(() => {
    refreshLedger();
  }, [refreshLedger]);

  // Apply: stage a PENDING artifact for the finding/issue, then apply it. Both
  // calls return the real Artifact; we flip the card off `apply`'s returned
  // `status` (never faked) and fold the result into the ledger. In browser QA
  // the invoke rejects → we surface an honest "never writes" preview instead.
  const onApplyFix = useCallback(
    async (issue: OrbIssue) => {
      setApplying(true);
      setRunError(null);
      try {
        const staged = await invoke<Artifact>('stage_artifact', {
          findingId: issue.findingId ?? null,
          issueId: issue.findingId ? null : issue.id,
        });
        const applied = await invoke<Artifact>('apply_artifact', { id: staged.id });
        setArtifact(applied);
        setArtifacts((cur) => mergeArtifact(cur, applied));
      } catch (e) {
        setRunError(String(e));
      } finally {
        setApplying(false);
      }
    },
    [],
  );

  // Revert: restore the verified pre-image and flip the card back to candidate.
  // Drives off the returned Artifact's `reverted` status; refuses silently-safe
  // if the backend rejects (sha mismatch surfaces the typed error to the user).
  const onRevertFix = useCallback(
    async (id: string) => {
      setReverting(true);
      setRunError(null);
      try {
        const reverted = await invoke<Artifact>('revert_artifact', { id });
        setArtifact((cur) => (cur && cur.id === id ? reverted : cur));
        setArtifacts((cur) => mergeArtifact(cur, reverted));
      } catch (e) {
        setRunError(String(e));
      } finally {
        setReverting(false);
      }
    },
    [],
  );

  const onToggleLedger = useCallback(() => {
    setLedgerOpen((open) => {
      // Opening the ledger pulls a fresh history so it never shows a stale trail.
      if (!open) void refreshLedger();
      return !open;
    });
  }, [refreshLedger]);

  const onDismiss = useCallback(() => {
    invoke('hide_overlay').catch(() => {});
  }, []);

  const findings = useMemo(() => deriveFindings(scene), [scene.verdicts]);
  const diagnosisId = scene.diagnosisId ?? 'diagnosis';

  return (
    <div className={`viz-root viz-phase-${scene.phase} viz-orb-map`} onDoubleClick={onDiscoveryHomeDoubleClick}>
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
          homeSignal={homeSignal}
          sceneBounds={sceneBounds}
          flyMode={flyMode}
          scaleRef={foldScale}
          onHover={onHover}
          onLeave={onLeave}
          onSelect={onSelect}
          onClear={onClear}
        />
      </Canvas>

      {flyMode && (
        <div
          role="status"
          style={{
            position: 'fixed',
            bottom: 18,
            left: '50%',
            transform: 'translateX(-50%)',
            zIndex: 20,
            padding: '6px 14px',
            borderRadius: 6,
            background: 'rgba(2,4,3,0.82)',
            border: '1px solid #1b6f3a',
            color: '#76ff9d',
            font: '11px/1 "SF Mono", Menlo, monospace',
            letterSpacing: '0.12em',
            textTransform: 'uppercase',
            pointerEvents: 'none',
            boxShadow: '0 0 18px rgba(118,255,157,0.18)',
          }}
        >
          ✈ free fly · W A S D move · Q / E roll · R / F up·down · drag to look · F or Esc to exit
        </div>
      )}

      <NavBar
        tab={tab}
        onTab={onTab}
        counts={{ habits: layout.nodes.filter((n) => n.kind === 'issue').length, radar: radarModel.agents.length }}
      />

      {/* ≡ roster toggle (top-left) + the left roster Sidebar. The roster lists
          every globe as a scannable list (radar agents nested by harness / habits
          by harness); a row click selects that globe via the shared selection. */}
      <button
        type="button"
        className={`wd-side-toggle${sidebarOpen ? ' is-open' : ''}`}
        aria-expanded={sidebarOpen}
        aria-controls="wd-roster"
        aria-label={sidebarOpen ? 'Collapse roster' : 'Open roster'}
        title="Roster"
        onClick={onToggleSidebar}
      >
        ☰
      </button>

      <Sidebar
        open={sidebarOpen}
        displayTab={displayTab}
        groups={rosterGroups}
        headerCount={rosterHeader}
        selectedId={selectedId}
        onPick={onPickRoster}
        onToggle={onToggleSidebar}
      />

      {/* The severity + harness emphasis filter, centred along the bottom (its own
          dock now — it replaced the removed StatusDeck). */}
      <FilterBar tab={tab} model={chromeModel} filter={emphasisFilter} onFilter={onFilter} />

      {/* Chrome is the Habits inspector (keys off node.issue/agent). On the radar
          tab the live selection flows to RadarSceneBody via selectedId; the radar
          detail panel is Phase 3, so keep the Habits inspector closed here rather
          than feeding it a radar node it cannot render. */}
      <Chrome
        scene={scene}
        model={chromeModel}
        tab={tab}
        hoveredNode={displayTab === 'radar' ? null : hoveredNode}
        selectedNode={displayTab === 'radar' ? null : selectedNode}
        focusStack={focusStack}
        running={Boolean(scene.running)}
        error={runError}
        fixPreview={fixPreview}
        loadingFix={loadingFix}
        artifact={artifact}
        artifacts={artifacts}
        applying={applying}
        reverting={reverting}
        ledgerOpen={ledgerOpen}
        onAsk={onAsk}
        onRequestFix={onRequestFix}
        onApplyFix={onApplyFix}
        onRevertFix={onRevertFix}
        onToggleLedger={onToggleLedger}
        onClearSelection={onClear}
        onDismiss={onDismiss}
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
