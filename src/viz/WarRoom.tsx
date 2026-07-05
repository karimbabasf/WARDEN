// WarRoom.tsx — the live RADAR fleet map AND the whole interface.
//
// RADAR is the hero: the war room renders the live agent forest only. The 3D layer
// (fresnel orbs, free-orbit camera, links, atmosphere) renders the `RadarSceneModel`
// built by Rust from real local-transcript signals; the DOM `Chrome` layer over it
// carries the HUD, the click-to-run diagnosis button + pipeline, the radar detail
// panel, the harness filter and the guardrail ledger. Nav switches between RADAR
// (this war room) and DOSSIER (the full-page profile overlay).
//
// Honest-viz holds throughout: every orb/link/flare maps to a computed signal, and
// off-Fugu runs degrade gracefully (no fabricated counts, verdicts or costs).

import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from 'react';
import { Canvas, useThree } from '@react-three/fiber';
import { EffectComposer, Bloom, Vignette } from '@react-three/postprocessing';
import { Environment, Lightformer } from '@react-three/drei';
import * as THREE from 'three';
import { invoke } from '@tauri-apps/api/core';
import type { Bridge, SceneState } from './bridge';
import type { LayoutNode, OrbIssue, OrbSceneModel } from './orbTypes';
import { StarCatalog } from './StarCatalog';
import { CameraRig } from './CameraRig';
import { Chrome, type Artifact, type FixPreview } from './chrome';
import type { RevealFinding } from './compositions/Reveal';
import { NavBar, PRIMARY_VIEW, type NavView } from './NavBar';
import { RadarForest } from './RadarConstellation';
import { RadarDetailPanel } from './RadarDetailPanel';
import { FilterBar } from './FilterBar';
import { Sidebar } from './Sidebar';
import { buildRadarRoster } from './rosterTree';
import { layoutRadarScene, isFlatAgent } from './radarLayout';
import { radarHarness } from './radarTheme';
import { openProfile, closeProfile, isProfileOpen } from './profile/mount';
import type { RadarAgent, RadarSceneModel } from './radarTypes';
import { type EmphasisFilter } from './emphasis';
import { subtreeBounds, type Bounds } from './cameraFraming';
import IntroVideo from './IntroVideo';
import { HandMode } from './gesture/HandMode';

const PlayerHost = lazy(() => import('./PlayerHost'));

const BG = '#020403';

export function frameloopFor(hidden: boolean): 'always' | 'never' {
  return hidden ? 'never' : 'always';
}

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

// Raw Rust error strings (HTTP bodies, 3-transport failure chains) are for logs,
// not the ask bar. Map the common failures to one plain sentence; anything else
// gets a firmly truncated tail so the console never floods.
export function humanizeBrainError(raw: string): string {
  const s = raw.toLowerCase();
  if (s.includes('401') || s.includes('403') || s.includes('unauthorized') || s.includes('api key')) {
    return 'The brain rejected the request — check your API key (WARDEN_BRAIN_API_KEY).';
  }
  if (s.includes('timed out') || s.includes('timeout')) {
    return 'The brain took too long to answer — try again (slow model or network).';
  }
  if (s.includes('connection') || s.includes('dns') || s.includes('sending request')) {
    return "Can't reach the brain — check your network or WARDEN_BRAIN_BASE_URL.";
  }
  const flat = raw.replace(/\s+/g, ' ').trim();
  return flat.length > 220 ? `${flat.slice(0, 200)}… (see logs)` : flat;
}

// The implicit prompt behind the "Diagnose my workflow" button — the diagnosis is
// now click-to-run, not typed, so this is the single question WARDEN always asks of
// the operator's own agent history.
export const WORKFLOW_QUERY = "what's wrong with how I use my agents?";

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

// The chrome's harness-filter model is derived from the live radar agents (one
// synthetic hub per real harness on screen) so the FilterBar's harness chips reflect
// exactly what the fleet contains — honest-viz, never a fabricated harness.
export function radarChromeModel(radarModel: RadarSceneModel): OrbSceneModel {
  const byHarness = new Map<string, { count: number; load: number }>();
  for (const agent of radarModel.agents) {
    const cur = byHarness.get(agent.harness) ?? { count: 0, load: 0 };
    cur.count += 1;
    cur.load += agent.contextTokens;
    byHarness.set(agent.harness, cur);
  }
  const agents = Array.from(byHarness, ([harness, meta]) => {
    const t = radarHarness(harness);
    return {
      id: harness,
      harness,
      label: t.label,
      glyph: t.glyph,
      color: t.color,
      sessions: meta.count,
      eventCount: 0,
      totalLoad: meta.load,
    };
  });
  return { agents, issues: [], links: [], guidance: { doItems: [], stopItems: [] } };
}

// The persistent scene shell — the SINGLE always-mounted void (background, fog,
// lights, Environment, starfield, the shared free-orbit CameraRig and the post
// stack) wrapping the live RADAR forest. RADAR is the only scene now, so the shell
// never swaps a child: the whole app is one continuous motion with no flicker. The
// Environment carries formers for every harness hue (Claude-emerald, Codex-violet,
// warm) so every gem keeps its glint.
function SceneShell({
  radarModel,
  selected,
  selectedId,
  hoveredId,
  emphasisFilter,
  focusBounds,
  homeSignal,
  onHover,
  onLeave,
  onSelect,
  onClear,
}: {
  radarModel: RadarSceneModel;
  selected: LayoutNode | null;
  selectedId: string | null;
  hoveredId: string | null;
  /** Active legend filter, forwarded to the forest for the colour-only dim/pop. */
  emphasisFilter: EmphasisFilter;
  /** Cinematic fly-to bounds for the shared CameraRig (null = overview/home). */
  focusBounds: Bounds | null;
  /** Monotonic signal that asks the shared CameraRig to return to home. */
  homeSignal: number;
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

  // RADAR renders at rest — no fold transition remains (there is no second scene to
  // cross-fade to), so the forest's fold scale is pinned to 1.
  const restScale = useRef(1);

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
        {/* Claude-tangerine, Codex-cyan + warm formers so every gem glints in its
            own hue without the void changing. */}
        <Lightformer form="rect" intensity={1.7} color="#ffcaa0" position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.4} color="#bfeaff" position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="rect" intensity={1.0} color="#ffd9b8" position={[0, -3, -4]} scale={[6, 4, 1]} />
        <Lightformer form="ring" intensity={1.1} color="#ffffff" position={[2, 4, 2]} scale={[2, 2, 1]} />
      </Environment>

      {/* Deep multi-layer star catalog — fine, dense, glacially drifting, and
          deliberately subordinate so the data reads first (see StarCatalog.tsx). */}
      <StarCatalog />

      <CameraRig selected={selected} focusBounds={focusBounds} homeSignal={homeSignal} />

      <RadarForest
        model={radarModel}
        selectedId={selectedId}
        hoveredId={hoveredId}
        emphasisFilter={emphasisFilter}
        scaleRef={restScale}
        onHover={onHover}
        onLeave={onLeave}
        onSelect={onSelect}
        onClear={onClear}
      />

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
  // The nav view — RADAR (this war room) or DOSSIER (the profile overlay). The
  // DOSSIER surface is a separate React root opened imperatively (profile/mount);
  // this bit only mirrors that open/closed state so the nav lights the right item.
  const [view, setView] = useState<NavView>(() => (isProfileOpen() ? 'dossier' : PRIMARY_VIEW));
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
  // ✕ both toggle it. Session-local (not persisted).
  const [sidebarOpen, setSidebarOpen] = useState(false);

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

  // Radar forest (live agents) — empty until the backend emits `radar_state`.
  const radarModel = useMemo<RadarSceneModel>(() => scene.radarScene ?? { agents: [], generatedAt: '' }, [scene.radarScene]);
  // Harness-filter model for the FilterBar (one hub per real harness on screen).
  const chromeModel = useMemo(() => radarChromeModel(radarModel), [radarModel]);
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
  const selectedNode = useMemo(() => radarLayout.nodes.find((n) => n.id === selectedId) ?? null, [radarLayout, selectedId]);
  const hoveredNode = useMemo(() => radarLayout.nodes.find((n) => n.id === hoveredId) ?? null, [radarLayout, hoveredId]);

  // Roster (left sidebar): the live radar agents grouped by harness with subagents
  // nested. Built from the SAME model the forest renders (honest-viz) so a row click
  // selects exactly that globe via the shared `selectedId`.
  const rosterGroups = useMemo(() => buildRadarRoster(radarModel.agents), [radarModel]);
  const rosterHeader = useMemo(() => {
    const n = radarModel.agents.length;
    const working = radarModel.agents.filter((a) => a.status === 'working').length;
    return `${n} ${n === 1 ? 'agent' : 'agents'} · ${working} working`;
  }, [radarModel]);

  // Radar detail-panel inputs: the selected live agent and its REAL children
  // (agents whose parentId === the selection). A flat agent yields []; the panel
  // then renders no roster (honest-viz — never a fabricated children list).
  const selectedRadarAgent = useMemo<RadarAgent | null>(
    () => (selectedId ? radarModel.agents.find((a) => a.id === selectedId) ?? null : null),
    [selectedId, radarModel],
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
  }, [selectedId, radarModel]);

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

  // Terminate a live agent: kill its process on this machine, back out of the dive,
  // and pull a fresh forest at once (the backend also pushes `radar_state`). A backend
  // refusal — e.g. a subagent has no process of its own — rejects, and the detail panel
  // renders the message inline. `invoke` also rejects in the browser QA harness (no
  // Tauri); the panel surfaces that the same way.
  const onTerminate = useCallback(
    async (id: string) => {
      await invoke('terminate_agent', { agentId: id });
      onClear();
      void fetchRadar();
    },
    [onClear, fetchRadar],
  );

  useEffect(() => {
    if (!active) return;
    fetchRadar(); // immediate on mount / on summon
    const id = window.setInterval(fetchRadar, RADAR_VISIBLE_PULL_MS); // fallback; push events remain the fast path
    return () => window.clearInterval(id);
  }, [active, fetchRadar]);

  // Nav: switch between RADAR (this war room) and DOSSIER (the profile overlay).
  // The DOSSIER surface is a separate React root driven imperatively; selecting it
  // opens the profile over the war room, selecting RADAR closes it back to the fleet.
  // `view` mirrors that open/closed state so the nav lights the active surface.
  const onView = useCallback((next: NavView) => {
    if (next === 'dossier') openProfile();
    else closeProfile();
    setView(next);
  }, []);

  // Keep the nav in sync if the DOSSIER overlay is dismissed from within itself (its
  // own Close button / the Escape + "D" hotkeys in main.ts). Sampling on focus/visibility
  // catches the common close paths without reaching into the profile root's internals.
  useEffect(() => {
    const sync = () => setView(isProfileOpen() ? 'dossier' : 'radar');
    window.addEventListener('focus', sync);
    document.addEventListener('visibilitychange', sync);
    return () => {
      window.removeEventListener('focus', sync);
      document.removeEventListener('visibilitychange', sync);
    };
  }, []);

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
        setRunError(humanizeBrainError(String(e)));
        bridge.ingest('diagnosis_run_failed', {});
      }
    },
    [bridge, scene.running],
  );

  // The "Diagnose my workflow" button fires the single implicit workflow query —
  // no typed prompt. Thin wrapper so the button's onClick carries no argument.
  const onDiagnose = useCallback(() => onAsk(WORKFLOW_QUERY), [onAsk]);

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
        // Opens further back so the (wide-spaced) fleet frames with room to breathe;
        // |pos| ≈ CameraRig's OVERVIEW_DIST so the first frame already sits at rest.
        camera={{ position: [4.8, 3.2, 11.7], fov: 46, near: 0.1, far: 140 }}
      >
        {/* One persistent shell wrapping the live RADAR forest — the void never
            remounts, so the whole app stays one continuous animation. */}
        <SceneShell
          radarModel={radarModel}
          selected={selectedNode}
          selectedId={selectedId}
          hoveredId={hoveredId}
          emphasisFilter={emphasisFilter}
          focusBounds={focusBounds}
          homeSignal={homeSignal}
          onHover={onHover}
          onLeave={onLeave}
          onSelect={onSelect}
          onClear={onClear}
        />
      </Canvas>

      <NavBar
        view={view}
        onView={onView}
        counts={{ radar: radarModel.agents.length }}
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
        groups={rosterGroups}
        headerCount={rosterHeader}
        selectedId={selectedId}
        onPick={onPickRoster}
        onToggle={onToggleSidebar}
      />

      {/* The harness emphasis filter, centred along the bottom (its own dock). */}
      <FilterBar model={chromeModel} filter={emphasisFilter} onFilter={onFilter} />

      {/* Hand mode — toggle a webcam pinch-to-orbit controller (camera off until asked).
          Fixed-positioned chrome; gated on `active` so minimising releases the camera. */}
      <HandMode active={active} />

      {/* Chrome — the screen-space cockpit: focus breadcrumb, the click-to-run
          diagnose dock (button + pipeline + coach brief), the guardrail ledger. The
          live radar selection flows to the RadarDetailPanel below, so Chrome's own
          orb inspector stays closed here (null nodes) — it renders the Forge fix
          preview only when a habits-issue node is fed to it (unit-tested path). */}
      <Chrome
        scene={scene}
        model={chromeModel}
        hoveredNode={null}
        selectedNode={null}
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
        onAsk={onDiagnose}
        onRequestFix={onRequestFix}
        onApplyFix={onApplyFix}
        onRevertFix={onRevertFix}
        onToggleLedger={onToggleLedger}
        onClearSelection={onClear}
        onDismiss={onDismiss}
        onPopFocus={onPopFocus}
        onClearFocus={onClearFocus}
      />

      {/* Radar detail panel — its own right-dock. Opens when a live globe is selected
          and the camera has dived in; the roster's jump-to flies to a child via
          onRadarJump (select + focus). */}
      <div className={`wd-inspector wd-radar-dock ${selectedRadarAgent ? 'is-open' : ''}`}>
        {selectedRadarAgent ? (
          <RadarDetailPanel
            agent={selectedRadarAgent}
            children={selectedRadarChildren}
            onJumpTo={onRadarJump}
            onClose={onClear}
            onTerminate={onTerminate}
            onDiagnose={onDiagnose}
            diagnosing={Boolean(scene.running)}
          />
        ) : null}
      </div>

      {/* Honest empty state — the radar is live and watching, there's just nothing
          running yet. Never reads as "broken": it says what to do to populate it. */}
      {radarModel.agents.length === 0 ? (
        <div className="wd-radar-empty" aria-live="polite">
          <span className="wd-radar-empty-pulse" aria-hidden />
          <span className="wd-radar-empty-title">Watching for live agents</span>
          <span className="wd-radar-empty-sub">Open Claude Code, Claude Desktop, or Codex and your sessions appear here.</span>
        </div>
      ) : null}

      {showIntro && <IntroVideo onEnded={() => setShowIntro(false)} />}
      {scene.phase === 'reveal' && (
        <Suspense fallback={null}>
          <PlayerHost
            kind="reveal"
            findings={findings}
            diagnosisId={diagnosisId}
            detectorOnly={Boolean((scene.diagnosis as { detector_only?: boolean } | undefined)?.detector_only)}
            onEnded={() => bridge.ingest('diagnosis_reveal_done', {})}
          />
        </Suspense>
      )}
    </div>
  );
}

export default WarRoom;
