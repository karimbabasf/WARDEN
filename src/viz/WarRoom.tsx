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
import { Canvas, useFrame, useThree } from '@react-three/fiber';
import { EffectComposer, Bloom, Vignette } from '@react-three/postprocessing';
import { Sparkles, Stars, Environment, Lightformer, Html, Billboard } from '@react-three/drei';
import * as THREE from 'three';
import { invoke } from '@tauri-apps/api/core';
import type { Bridge, SceneState } from './bridge';
import { harnessTheme } from './harnessTheme';
import { layoutOrbScene } from './orbLayout';
import type { LayoutNode, OrbIssue, OrbLayout, OrbSceneModel } from './orbTypes';
import { Orb } from './Orb';
import { CameraRig } from './CameraRig';
import { Chrome, type FixPreview } from './chrome';
import type { RevealFinding } from './compositions/Reveal';
import { NavBar, type ConstellationTab } from './NavBar';
import { RadarSceneBody } from './RadarConstellation';
import { RadarDetailPanel } from './RadarDetailPanel';
import { layoutRadarScene, isFlatAgent } from './radarLayout';
import type { RadarAgent, RadarSceneModel } from './radarTypes';

const PlayerHost = lazy(() => import('./PlayerHost'));

const BG = '#020403';

export function frameloopFor(hidden: boolean): 'always' | 'never' {
  return hidden ? 'never' : 'always';
}

export function activeFor(summoned: boolean | undefined, visHidden: boolean): boolean {
  return Boolean(summoned) || !visHidden;
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

// soft round sprite for the travelling link dots
let linkDotCache: THREE.Texture | null = null;
function linkDotTexture(): THREE.Texture {
  if (linkDotCache) return linkDotCache;
  const s = 48;
  const c = document.createElement('canvas');
  c.width = c.height = s;
  const ctx = c.getContext('2d')!;
  const g = ctx.createRadialGradient(s / 2, s / 2, 0, s / 2, s / 2, s / 2);
  g.addColorStop(0, 'rgba(255,255,255,1)');
  g.addColorStop(0.4, 'rgba(255,255,255,0.75)');
  g.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, s, s);
  linkDotCache = new THREE.CanvasTexture(c);
  linkDotCache.needsUpdate = true;
  return linkDotCache;
}

// ── links: faint harness-tinted lines + an energy dot flowing issue → hub, so
// every issue visibly belongs to its agent (the grouping cue, since the orbs
// themselves are severity-coloured). ─────────────────────────────────────────
function AnimatedLinks({ layout }: { layout: OrbLayout }) {
  const byId = useMemo(() => new Map(layout.nodes.map((n) => [n.id, n])), [layout]);
  const links = useMemo(() => layout.links.filter((l) => byId.has(l.source) && byId.has(l.target)), [layout, byId]);

  const lineGeo = useMemo(() => {
    const positions = new Float32Array(links.length * 6);
    const colors = new Float32Array(links.length * 6);
    links.forEach((link, i) => {
      const hub = byId.get(link.source)!;
      const iss = byId.get(link.target)!;
      positions.set([hub.position.x, hub.position.y, hub.position.z, iss.position.x, iss.position.y, iss.position.z], i * 6);
      const c = new THREE.Color(harnessTheme(hub.harness).color);
      colors.set([c.r, c.g, c.b, c.r * 0.45, c.g * 0.45, c.b * 0.45], i * 6);
    });
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3));
    g.setAttribute('color', new THREE.BufferAttribute(colors, 3));
    return g;
  }, [links, byId]);

  const dotGeo = useMemo(() => {
    const positions = new Float32Array(links.length * 3);
    const colors = new Float32Array(links.length * 3);
    links.forEach((link, i) => {
      const c = new THREE.Color(harnessTheme(byId.get(link.source)!.harness).color);
      colors.set([c.r, c.g, c.b], i * 3);
    });
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.BufferAttribute(positions, 3));
    g.setAttribute('color', new THREE.BufferAttribute(colors, 3));
    return g;
  }, [links, byId]);

  const meta = useMemo(
    () => links.map((link, i) => ({ hub: byId.get(link.source)!.position, iss: byId.get(link.target)!.position, phase: (i * 0.37) % 1 })),
    [links, byId],
  );

  const lineMat = useRef<THREE.LineBasicMaterial>(null);
  const dotTex = useMemo(() => linkDotTexture(), []);

  useEffect(() => () => { lineGeo.dispose(); dotGeo.dispose(); }, [lineGeo, dotGeo]);

  useFrame((state) => {
    const t = state.clock.elapsedTime;
    const attr = dotGeo.getAttribute('position') as THREE.BufferAttribute;
    for (let i = 0; i < meta.length; i++) {
      const m = meta[i];
      const tt = (t * 0.4 + m.phase) % 1; // issue → hub
      attr.setXYZ(
        i,
        m.iss.x + (m.hub.x - m.iss.x) * tt,
        m.iss.y + (m.hub.y - m.iss.y) * tt,
        m.iss.z + (m.hub.z - m.iss.z) * tt,
      );
    }
    attr.needsUpdate = true;
    if (lineMat.current) lineMat.current.opacity = 0.22 + Math.sin(t * 1.4) * 0.05;
  });

  if (links.length === 0) return null;
  return (
    <group>
      <lineSegments geometry={lineGeo}>
        <lineBasicMaterial ref={lineMat} vertexColors transparent opacity={0.24} toneMapped={false} blending={THREE.AdditiveBlending} />
      </lineSegments>
      <points geometry={dotGeo}>
        <pointsMaterial
          vertexColors
          size={0.17}
          map={dotTex}
          transparent
          opacity={0.95}
          sizeAttenuation
          depthWrite={false}
          blending={THREE.AdditiveBlending}
          toneMapped={false}
        />
      </points>
    </group>
  );
}

// Faint camera-facing ring bounding each agent's cluster — "everything inside
// here is this agent" — sized to the cluster's dynamic extent.
function TerritoryRings({ hubs }: { hubs: LayoutNode[] }) {
  return (
    <>
      {hubs.map((h) => {
        const r = h.territoryRadius ?? 2;
        return (
          <Billboard key={`terr-${h.id}`} position={[h.position.x, h.position.y, h.position.z]}>
            <mesh>
              <ringGeometry args={[r * 0.975, r, 96]} />
              <meshBasicMaterial
                color={harnessTheme(h.harness).color}
                transparent
                opacity={0.12}
                side={THREE.DoubleSide}
                depthWrite={false}
                blending={THREE.AdditiveBlending}
                toneMapped={false}
              />
            </mesh>
          </Billboard>
        );
      })}
    </>
  );
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

function Scene({
  layout,
  selected,
  selectedId,
  hoveredId,
  onHover,
  onLeave,
  onSelect,
  onClear,
}: {
  layout: OrbLayout;
  selected: LayoutNode | null;
  selectedId: string | null;
  hoveredId: string | null;
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

  const selectedAgent = selected?.agentId;
  const hubs = useMemo(() => layout.nodes.filter((n) => n.kind === 'hub'), [layout]);

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
        <Lightformer form="rect" intensity={1.8} color="#bfffe0" position={[-5, 3, -3]} scale={[7, 7, 1]} />
        <Lightformer form="rect" intensity={1.3} color="#3dffa0" position={[5, 1, -4]} scale={[6, 6, 1]} />
        <Lightformer form="ring" intensity={1.2} color="#cfe9d8" position={[2, 4, 2]} scale={[2, 2, 1]} />
      </Environment>

      {/* Real starfield (pulled in close + dense so it actually reads as a sky)
          plus a few slow near motes for parallax depth. */}
      <Stars radius={34} depth={46} count={4200} factor={5} saturation={0} fade speed={0.1} />
      <Sparkles count={45} scale={[26, 15, 24]} size={1.5} speed={0.05} opacity={0.26} color="#bfeaff" />

      <CameraRig selected={selected} />

      <group onPointerMissed={onClear}>
        <TerritoryRings hubs={hubs} />
        <AnimatedLinks layout={layout} />
        {layout.nodes.map((node, i) => (
          <Orb
            key={node.id}
            node={node}
            selected={selectedId === node.id}
            hovered={hoveredId === node.id}
            dimmed={Boolean(selectedId && selectedId !== node.id && node.agentId !== selectedAgent)}
            appearDelay={Math.min(i * 0.045, 0.6)}
            onHover={onHover}
            onLeave={onLeave}
            onSelect={onSelect}
          />
        ))}
      </group>
      <HubLabels hubs={hubs} />

      {/* multisampling AA on the composer input stops the thin bright lattice
          lines from sub-pixel shimmering into the bloom pass (the flicker). High
          smoothing + a higher threshold keep the bloom stable + calm. */}
      <EffectComposer multisampling={4}>
        <Bloom intensity={0.92} luminanceThreshold={0.28} luminanceSmoothing={0.95} mipmapBlur radius={0.74} />
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
  const [tab, setTab] = useState<ConstellationTab>('habits');
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const [fixPreview, setFixPreview] = useState<FixPreview | undefined>();
  const [loadingFix, setLoadingFix] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);
  const active = activeFor(scene.summoned, visHidden);
  const introPlayed = useRef(!document.hidden);
  const [showIntro, setShowIntro] = useState(false);

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
    return () => document.removeEventListener('visibilitychange', onVis);
  }, []);

  const model = useMemo(() => scene.orbScene ?? fallbackOrbScene(scene), [scene.orbScene, scene.candidates]);
  const layout = useMemo(() => layoutOrbScene(model), [model]);
  // Radar forest (live agents) — empty until the backend emits `radar_state`.
  const radarModel = useMemo<RadarSceneModel>(() => scene.radarScene ?? { agents: [], generatedAt: '' }, [scene.radarScene]);
  const activeLayout = tab === 'radar' ? layoutRadarScene(radarModel) : layout;
  const selectedNode = useMemo(() => activeLayout.nodes.find((n) => n.id === selectedId) ?? null, [activeLayout, selectedId]);
  const hoveredNode = useMemo(() => activeLayout.nodes.find((n) => n.id === hoveredId) ?? null, [activeLayout, hoveredId]);

  // Radar detail-panel inputs: the selected live agent and its REAL children
  // (agents whose parentId === the selection). A flat agent yields []; the panel
  // then renders no roster (honest-viz — never a fabricated children list).
  const selectedRadarAgent = useMemo<RadarAgent | null>(
    () => (tab === 'radar' && selectedId ? radarModel.agents.find((a) => a.id === selectedId) ?? null : null),
    [tab, selectedId, radarModel],
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
    setSelectedId(node.id);
    setFixPreview(undefined);
  }, []);
  const onClear = useCallback(() => {
    setSelectedId(null);
    setFixPreview(undefined);
  }, []);

  // Roster jump-to: select a child agent by id, which dives the shared CameraRig
  // onto that globe (selectedId → selectedNode → focus) and re-points the panel.
  const onRadarJump = useCallback((id: string) => setSelectedId(id), []);

  // Switching constellations clears the per-scene hover/selection (the two scenes
  // have disjoint node-id spaces) so the inspector never points at a stale node.
  const onTab = useCallback((next: ConstellationTab) => {
    setTab((cur) => {
      if (cur === next) return cur;
      setSelectedId(null);
      setHoveredId(null);
      setFixPreview(undefined);
      return next;
    });
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
        camera={{ position: [3.6, 2.4, 8.8], fov: 46, near: 0.1, far: 120 }}
      >
        {tab === 'radar' ? (
          <RadarSceneBody
            model={radarModel}
            selectedId={selectedId}
            hoveredId={hoveredId}
            onHover={onHover}
            onLeave={onLeave}
            onSelect={onSelect}
            onClear={onClear}
          />
        ) : (
          <Scene
            layout={layout}
            selected={selectedNode}
            selectedId={selectedId}
            hoveredId={hoveredId}
            onHover={onHover}
            onLeave={onLeave}
            onSelect={onSelect}
            onClear={onClear}
          />
        )}
      </Canvas>

      <NavBar tab={tab} onTab={onTab} />

      {/* Chrome is the Habits inspector (keys off node.issue/agent). On the radar
          tab the live selection flows to RadarSceneBody via selectedId; the radar
          detail panel is Phase 3, so keep the Habits inspector closed here rather
          than feeding it a radar node it cannot render. */}
      <Chrome
        scene={scene}
        model={model}
        hoveredNode={tab === 'radar' ? null : hoveredNode}
        selectedNode={tab === 'radar' ? null : selectedNode}
        running={Boolean(scene.running)}
        error={runError}
        fixPreview={fixPreview}
        loadingFix={loadingFix}
        onAsk={onAsk}
        onRequestFix={onRequestFix}
        onClearSelection={onClear}
        onDismiss={onDismiss}
      />

      {/* Radar detail panel — its own right-dock (the Chrome inspector is Habits-
          only). Opens when a radar globe is selected and the camera has dived in;
          the roster's jump-to flies to a child via onRadarJump (select + focus). */}
      <div className={`wd-inspector wd-radar-dock ${tab === 'radar' && selectedRadarAgent ? 'is-open' : ''}`}>
        {selectedRadarAgent ? (
          <RadarDetailPanel
            agent={selectedRadarAgent}
            children={selectedRadarChildren}
            onJumpTo={onRadarJump}
            onClose={onClear}
          />
        ) : null}
      </div>

      {showIntro && (
        <Suspense fallback={null}>
          <PlayerHost kind="intro" findings={[]} diagnosisId={diagnosisId} onEnded={() => setShowIntro(false)} />
        </Suspense>
      )}
      {scene.phase === 'reveal' && (
        <Suspense fallback={null}>
          <PlayerHost kind="reveal" findings={findings} diagnosisId={diagnosisId} />
        </Suspense>
      )}
    </div>
  );
}

export default WarRoom;
