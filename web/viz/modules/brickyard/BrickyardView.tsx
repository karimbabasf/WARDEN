// BrickyardView.tsx: the Brickyard. Every root agent gets a workbench, every helper
// (subagent or teammate) a minifig at its lead's bench, and every write-type tool call
// a brick on a real LDraw set. The build log is polled from Rust (`get_build_log`), so
// every brick on screen is a call the agent actually made.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Canvas, useFrame, useThree } from '@react-three/fiber';
import { Environment, Lightformer, OrbitControls } from '@react-three/drei';
import * as THREE from 'three';
import { invoke } from '@tauri-apps/api/core';
import type { RadarAgent } from '@/viz/shared/types/radarTypes';
import { harnessColor } from '@/viz/shared/theme/harnessColors';
import { BackgroundFrameTick } from '@/viz/shared/scene/BackgroundFrameTick';
import { StarCatalog } from '@/viz/shared/scene/StarCatalog';
import { normalizeBuildLog, type BuildStep } from './buildLog';
import { distinctOffsets } from './catalog';
import { BENCH_D, BENCH_W, BenchRig, type BenchActor, type BenchStats, type BrickPick } from './bench';
import { BrickyardPanel } from './BrickyardPanel';
import { nameOfAgent } from './names';

const BG = '#05070c';
/** Cap on benches drawn at once; the rest stay on the fleet rail. */
export const MAX_BENCHES = 12;
const POLL_MS = 1000;
const GAP_X = BENCH_W + 1.6;
const GAP_Z = BENCH_D + 1.8;

export type FetchLog = (agentId: string, after: number | null) => Promise<BuildStep[]>;

export const fetchBuildLog: FetchLog = async (agentId, after) =>
  normalizeBuildLog(await invoke('get_build_log', after === null ? { agentId } : { agentId, after }));

/** The root an agent belongs to: walk parents until there are none. */
export function rootOf(agent: RadarAgent, byId: Map<string, RadarAgent>): RadarAgent {
  let cur = agent;
  for (let guard = 0; guard < 16 && cur.parentId; guard++) {
    const up = byId.get(cur.parentId);
    if (!up) break;
    cur = up;
  }
  return cur;
}

const nameOf = nameOfAgent;

type BenchPlan = { lead: RadarAgent; helpers: RadarAgent[] };

/** One bench per root, in start order; helpers grouped under their root. */
export function planBenches(agents: RadarAgent[]): BenchPlan[] {
  const byId = new Map(agents.map((a) => [a.id, a]));
  const benches = new Map<string, BenchPlan>();
  const roots = agents.filter((a) => !a.parentId || !byId.has(a.parentId));
  roots.sort((a, b) => (a.startedAt < b.startedAt ? -1 : a.startedAt > b.startedAt ? 1 : a.id.localeCompare(b.id)));
  for (const r of roots.slice(0, MAX_BENCHES)) benches.set(r.id, { lead: r, helpers: [] });
  for (const a of agents) {
    if (benches.has(a.id)) continue;
    const root = rootOf(a, byId);
    benches.get(root.id)?.helpers.push(a);
  }
  return [...benches.values()];
}

function benchPosition(i: number, n: number): THREE.Vector3 {
  const cols = Math.max(1, Math.ceil(Math.sqrt(n)));
  const rows = Math.ceil(n / cols);
  const c = i % cols;
  const r = Math.floor(i / cols);
  return new THREE.Vector3((c - (cols - 1) / 2) * GAP_X, 0, (r - (rows - 1) / 2) * GAP_Z);
}

function actorsFor(plan: BenchPlan): BenchActor[] {
  return [
    { id: plan.lead.id, role: 'lead', status: plan.lead.status, name: nameOf(plan.lead) },
    ...plan.helpers.map((h) => ({ id: h.id, role: 'helper' as const, status: h.status, name: nameOf(h) })),
  ];
}

/** The camera goal: the whole yard, or one bench up close. */
type CamGoal = { target: THREE.Vector3; position: THREE.Vector3 } | null;

function CameraFlight({ goal, controls }: { goal: CamGoal; controls: React.RefObject<any> }) {
  const flying = useRef(false);
  useEffect(() => {
    flying.current = goal !== null;
  }, [goal]);
  useEffect(() => {
    const c = controls.current;
    if (!c) return;
    // A drag takes the camera; the flight gives it up rather than fight the hand.
    const stop = () => {
      flying.current = false;
    };
    c.addEventListener('start', stop);
    return () => c.removeEventListener('start', stop);
  }, [controls]);
  useFrame((state, dt) => {
    const c = controls.current;
    if (!c || !goal || !flying.current) return;
    const k = 1 - Math.exp(-dt * 4.2);
    c.target.lerp(goal.target, k);
    state.camera.position.lerp(goal.position, k);
    c.update();
    if (state.camera.position.distanceTo(goal.position) < 0.01 && c.target.distanceTo(goal.target) < 0.01) {
      flying.current = false;
    }
  });
  return null;
}

function YardScene({
  rigs,
  benchCount,
  goal,
  onPick,
  onMiss,
  onBook,
}: {
  rigs: Map<string, BenchRig>;
  benchCount: number;
  goal: CamGoal;
  onPick: (p: BrickPick) => void;
  onMiss: () => void;
  onBook: (benchId: string) => void;
}) {
  const controls = useRef<any>(null);
  const { gl } = useThree();
  useEffect(() => {
    gl.setPixelRatio(Math.min(window.devicePixelRatio, 2));
    gl.toneMapping = THREE.ACESFilmicToneMapping;
    gl.toneMappingExposure = 1.2;
    gl.shadowMap.enabled = true;
    gl.shadowMap.type = THREE.PCFSoftShadowMap;
  }, [gl]);

  useFrame((state, dt) => {
    if (import.meta.env.DEV) (window as unknown as { __gl: unknown }).__gl = state.gl.info;
    const ms = Math.min(dt * 1000, 100);
    const now = performance.now();
    for (const r of rigs.values()) r.update(ms, now);
  });

  const extent = Math.max(GAP_X, Math.ceil(Math.sqrt(Math.max(1, benchCount))) * GAP_X);

  const onPointerDown = useCallback(
    (e: any) => {
      e.stopPropagation();
      for (const r of rigs.values()) {
        if (r.ownsBook(e.object)) {
          onBook(r.id);
          return;
        }
      }
      for (const r of rigs.values()) {
        const p = r.pick(e.object, e.faceIndex);
        if (p) {
          onPick(p);
          return;
        }
      }
    },
    [rigs, onPick, onBook],
  );

  return (
    <>
      <color attach="background" args={[BG]} />
      <fogExp2 attach="fog" args={[BG, 0.018]} />
      <hemisphereLight args={[0xd8e6ff, 0x1a1d26, 1.15]} />
      <directionalLight
        position={[6, 11, 7]}
        intensity={2.4}
        color="#fff1e2"
        castShadow
        shadow-mapSize={[2048, 2048]}
        shadow-camera-left={-extent}
        shadow-camera-right={extent}
        shadow-camera-top={extent}
        shadow-camera-bottom={-extent}
        shadow-camera-far={60}
        shadow-bias={-0.0004}
        shadow-normalBias={0.02}
      />
      <directionalLight position={[-7, 4, -3]} intensity={0.6} color="#bfd6ff" />
      <Environment resolution={64}>
        <Lightformer form="rect" intensity={1.2} color="#ffe2c4" position={[-4, 4, 3]} scale={[6, 6, 1]} />
        <Lightformer form="rect" intensity={0.9} color="#cfe4ff" position={[5, 2, -3]} scale={[6, 4, 1]} />
      </Environment>
      <StarCatalog />
      {/* The rigs are imperative objects; one group catches clicks on any brick. */}
      <group onPointerDown={onPointerDown} onPointerMissed={onMiss}>
        {[...rigs.values()].map((r) => (
          <primitive key={r.id} object={r.group} />
        ))}
      </group>
      <OrbitControls
        ref={controls}
        makeDefault
        enableDamping
        dampingFactor={0.08}
        minDistance={2}
        maxDistance={60}
        maxPolarAngle={Math.PI * 0.47}
      />
      <CameraFlight goal={goal} controls={controls} />
    </>
  );
}

export function BrickyardView({
  agents,
  frameloop,
  active,
  selectedId,
  onSelect,
  fetchLog = fetchBuildLog,
}: {
  agents: RadarAgent[];
  frameloop: 'always' | 'demand' | 'never';
  /** Polling runs only while the window is on screen. */
  active: boolean;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
  fetchLog?: FetchLog;
}) {
  const reducedMotion = useMemo(
    () => typeof window !== 'undefined' && !!window.matchMedia?.('(prefers-reduced-motion: reduce)').matches,
    [],
  );
  const plans = useMemo(() => planBenches(agents), [agents]);
  const benchIds = plans.map((p) => p.lead.id).join('|');
  const [rigs, setRigs] = useState<Map<string, BenchRig>>(() => new Map());
  const rigsRef = useRef(rigs);
  rigsRef.current = rigs;

  // Benches come and go with root agents. A rig is created once per root and kept.
  useEffect(() => {
    const offsets = distinctOffsets(plans.map((p) => p.lead.id));
    setRigs((cur) => {
      const next = new Map<string, BenchRig>();
      let changed = false;
      plans.forEach((p, i) => {
        let rig = cur.get(p.lead.id);
        if (!rig) {
          rig = new BenchRig(
            { id: p.lead.id, role: 'lead', status: p.lead.status, name: nameOf(p.lead) },
            harnessColor(p.lead.harness).hue,
            reducedMotion,
            offsets.get(p.lead.id) ?? 0,
          );
          changed = true;
        }
        rig.group.position.copy(benchPosition(i, plans.length));
        next.set(p.lead.id, rig);
      });
      for (const [id, rig] of cur) {
        if (!next.has(id)) {
          rig.dispose();
          changed = true;
        }
      }
      return changed || next.size !== cur.size ? next : cur;
    });
    // `benchIds` is the identity of the bench set; statuses flow through setActors.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [benchIds, reducedMotion]);

  useEffect(() => {
    for (const p of plans) void rigs.get(p.lead.id)?.setActors(actorsFor(p));
  }, [plans, rigs]);

  // Dev only: the rigs on `window`, so a browser pass can read bench state directly.
  useEffect(() => {
    if (import.meta.env.DEV) (window as unknown as { __brickyard: unknown }).__brickyard = rigs;
  }, [rigs]);

  useEffect(() => () => {
    for (const r of rigsRef.current.values()) r.dispose();
  }, []);

  // The build log poll: every actor at every bench, cursor-based, so each call only
  // carries what is new. Sequential on purpose: the store is one SQLite connection.
  const plansRef = useRef(plans);
  plansRef.current = plans;
  useEffect(() => {
    if (!active) return;
    let stopped = false;
    let timer = 0;
    const poll = async () => {
      for (const p of plansRef.current) {
        const rig = rigsRef.current.get(p.lead.id);
        if (!rig) continue;
        await rig.whenReady;
        const batch: { actor: string; steps: BuildStep[] }[] = [];
        for (const a of [p.lead, ...p.helpers]) {
          try {
            const steps = await fetchLog(a.id, rig.lastSeq(a.id));
            if (steps.length > 0) batch.push({ actor: a.id, steps });
          } catch {
            /* no backend (browser sandbox) or a transient error: the bench keeps what it has */
          }
          if (stopped) return;
        }
        if (batch.length > 0) await rig.ingest(batch);
      }
      if (!stopped) timer = window.setTimeout(poll, POLL_MS);
    };
    void poll();
    return () => {
      stopped = true;
      window.clearTimeout(timer);
    };
  }, [active, fetchLog, benchIds]);

  // Which bench the selection lives on (a helper selects its lead's bench).
  const byId = useMemo(() => new Map(agents.map((a) => [a.id, a])), [agents]);
  const selectedAgent = selectedId ? byId.get(selectedId) ?? null : null;
  const selectedBenchId = selectedAgent ? rootOf(selectedAgent, byId).id : null;
  const selectedRig = selectedBenchId ? rigs.get(selectedBenchId) ?? null : null;
  const selectedPlan = plans.find((p) => p.lead.id === selectedBenchId) ?? null;

  const [pick, setPick] = useState<BrickPick | null>(null);
  // A click on the book flies in to read it; any other click or bench change leaves.
  const [bookOf, setBookOf] = useState<string | null>(null);
  const reading = bookOf !== null && bookOf === selectedBenchId;
  const shelfPick = pick?.shelved && pick.benchId === selectedBenchId ? pick.setIndex : null;
  const goal = useMemo<CamGoal>(() => {
    // A finished set picked off the shelf: fly in close and look at it alone.
    if (reading && selectedRig) return selectedRig.bookView();
    const shelfAt = selectedRig && shelfPick !== null ? selectedRig.shelfFocus(shelfPick) : null;
    if (shelfAt) {
      return { target: shelfAt.clone().add(new THREE.Vector3(0.3, 0, 0)), position: shelfAt.clone().add(new THREE.Vector3(1.0, 0.9, 2.0)) };
    }
    if (selectedRig) {
      const target = selectedRig.focusPoint();
      return { target: target.clone().add(new THREE.Vector3(0.6, 0, 0)), position: target.clone().add(new THREE.Vector3(1.9, 3.4, 6.2)) };
    }
    const n = Math.max(1, plans.length);
    const cols = Math.ceil(Math.sqrt(n));
    const rows = Math.ceil(n / cols);
    const span = Math.max(cols * GAP_X, rows * GAP_Z * 1.3);
    return { target: new THREE.Vector3(0, 0.3, 0.6), position: new THREE.Vector3(span * 0.08, span * 0.48 + 2, span * 0.68 + 2.5) };
    // `benchIds` changes the yard's size.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedRig, benchIds, shelfPick, reading]);

  const onPick = useCallback(
    (p: BrickPick) => {
      setPick(p);
      setBookOf(null);
      if (p.benchId !== selectedBenchId) onSelect(p.benchId);
    },
    [onSelect, selectedBenchId],
  );
  useEffect(() => setPick((cur) => (cur && cur.benchId !== selectedBenchId ? null : cur)), [selectedBenchId]);

  // Stats for the panel, re-read whenever the selected rig reports a change.
  const [stats, setStats] = useState<BenchStats | null>(null);
  useEffect(() => {
    if (!selectedRig) {
      setStats(null);
      return;
    }
    let raf = 0;
    const read = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => setStats(selectedRig.stats()));
    };
    selectedRig.subscribe(read);
    void selectedRig.whenReady.then(read);
    return () => {
      cancelAnimationFrame(raf);
      selectedRig.subscribe(null);
    };
  }, [selectedRig]);

  return (
    <div className={`by-root${selectedRig ? ' has-dock' : ''}`}>
      <Canvas
        dpr={[1, 1.5]}
        frameloop={frameloop}
        shadows
        gl={{ antialias: true, alpha: false, powerPreference: 'high-performance' }}
        camera={{ position: [4, 9, 14], fov: 40, near: 0.05, far: 200 }}
      >
        {/* Paced at 60 even when blurred: a build is motion, and 30 read as lag. */}
        <BackgroundFrameTick paced={frameloop === 'demand'} fps={60} />
        <YardScene
          rigs={rigs}
          benchCount={plans.length}
          goal={goal}
          onPick={onPick}
          onMiss={() => {
            setPick(null);
            setBookOf(null);
          }}
          onBook={(id) => {
            setPick(null);
            setBookOf(id);
            if (id !== selectedBenchId) onSelect(id);
          }}
        />
      </Canvas>

      <BenchTags plans={plans} selectedId={selectedBenchId} onSelect={onSelect} />

      <div className={`wd-inspector by-dock ${selectedRig && selectedPlan ? 'is-open' : ''}`}>
        {selectedRig && selectedPlan ? (
          <BrickyardPanel
            rig={selectedRig}
            lead={selectedPlan.lead}
            helpers={selectedPlan.helpers}
            stats={stats}
            pick={pick}
            onPickStep={(step) => setPick(step)}
            onClose={() => onSelect(null)}
          />
        ) : null}
      </div>

      {plans.length === 0 ? (
        <div className="wd-radar-void" aria-live="polite">
          <span className="wd-radar-void-pulse" aria-hidden />
          <span className="wd-radar-void-title">The yard is empty</span>
          <span className="wd-radar-void-sub">Start a Claude Code or Codex session and a bench opens for it.</span>
        </div>
      ) : null}
    </div>
  );
}

/** A strip of bench names along the bottom: which bench is whose, and a way to fly there. */
function BenchTags({
  plans,
  selectedId,
  onSelect,
}: {
  plans: BenchPlan[];
  selectedId: string | null;
  onSelect: (id: string | null) => void;
}) {
  if (plans.length === 0) return null;
  return (
    <nav className="by-tags" aria-label="Benches">
      {plans.map((p) => {
        const hc = harnessColor(p.lead.harness);
        return (
          <button
            key={p.lead.id}
            type="button"
            className={`by-tag${p.lead.id === selectedId ? ' is-on' : ''}${p.lead.status === 'awaiting' ? ' is-awaiting' : ''}`}
            style={{ ['--harness' as string]: hc.hue }}
            onClick={() => onSelect(p.lead.id === selectedId ? null : p.lead.id)}
          >
            <span className="by-tag-glyph" aria-label={hc.label}>
              {hc.glyph}
            </span>
            <span className="by-tag-name">{nameOf(p.lead)}</span>
            {p.helpers.length > 0 ? <span className="by-tag-count">+{p.helpers.length}</span> : null}
          </button>
        );
      })}
    </nav>
  );
}
