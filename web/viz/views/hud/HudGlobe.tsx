// HudGlobe.tsx: one agent, as the same body the war room draws, at 40px.
//
// Deliberately the SAME family as the constellation: an additive line lattice with the
// harness's own gyro cradle and brand heart at its centre (`AgentCore`, shared). A HUD
// that invented a second visual language for the same object would make the two screens
// read as two products.
//
// The three signals are the constellation's three, unchanged, because they are the ones
// that are honest here too:
//   SIZE       context occupancy: a near-full agent is a bigger body
//   BRIGHTNESS liveness: working blazes, idle sits dim
//   ALERT      awaiting strobes crimson on `radarAlert`'s beat, so the HUD and the war
//              room flash together rather than looking like two alarms disagreeing.

import { useEffect, useMemo, useRef } from 'react';
import { useFrame } from '@react-three/fiber';
import * as THREE from 'three';
import { AgentCore } from '@/viz/shared/scene/AgentCore';
import { radarHarness } from '@/viz/modules/radar/radarTheme';
import { ALERT_HEX, alertBlink, alertWhiteMix } from '@/viz/modules/radar/radarAlert';
import type { RadarStatus } from '@/viz/shared/types/radarTypes';
import { hudGlowTexture } from './hudGlow';

const WHITE = new THREE.Color('#ffffff');
const ALERT = new THREE.Color(ALERT_HEX);

/** Body radius in CSS px, read off context occupancy exactly as the radar does. */
export function hudGlobeRadius(fillPct: number): number {
  const f = Number.isFinite(fillPct) ? Math.max(0, Math.min(1, fillPct)) : 0;
  return 15 + f * 5;
}

/** The eased liveness target: how hot this body burns. */
export function hudLiveness(status: RadarStatus): number {
  if (status === 'working') return 1;
  if (status === 'awaiting') return 0.55;
  return 0.16;
}

export function HudGlobe({
  harness,
  status,
  fillPct,
  hovered,
  reduced,
}: {
  harness: string;
  status: RadarStatus;
  fillPct: number;
  hovered: boolean;
  reduced: boolean;
}) {
  const body = useRef<THREE.Group>(null!);
  const outerMat = useRef<THREE.LineBasicMaterial>(null!);
  const innerMat = useRef<THREE.LineBasicMaterial>(null!);
  const haloMat = useRef<THREE.SpriteMaterial>(null!);
  const halo = useRef<THREE.Sprite>(null!);

  const base = useMemo(() => new THREE.Color(radarHarness(harness).color), [harness]);
  const outerGeo = useMemo(() => new THREE.WireframeGeometry(new THREE.IcosahedronGeometry(1, 1)), []);
  const innerGeo = useMemo(() => new THREE.WireframeGeometry(new THREE.IcosahedronGeometry(0.55, 0)), []);
  const glow = useMemo(() => hudGlowTexture(), []);
  useEffect(() => () => { outerGeo.dispose(); innerGeo.dispose(); }, [outerGeo, innerGeo]);

  // Live colour, recomputed per frame only when awaiting (the strobe is the one signal
  // that changes between frames); everything else damps toward a fixed target.
  const tint = useMemo(() => base.clone(), [base]);
  const sim = useRef({ live: hudLiveness(status), lift: 0 });

  useFrame((state, dtRaw) => {
    const dt = Math.min(dtRaw, 0.05);
    const t = state.clock.elapsedTime;
    const s = sim.current;
    const k = 1 - Math.exp(-6 * dt);
    s.live = THREE.MathUtils.lerp(s.live, hudLiveness(status), k);
    s.lift = THREE.MathUtils.lerp(s.lift, hovered ? 1 : 0, k);

    if (!reduced) {
      body.current.rotation.y += dt * (status === 'working' ? 0.42 : 0.16);
      body.current.rotation.x += dt * 0.06;
    }

    const alerting = status === 'awaiting';
    const blink = alerting ? alertBlink(t, reduced) : 0;
    tint.copy(alerting ? ALERT : base).lerp(WHITE, alerting ? alertWhiteMix(blink) : s.live * 0.4);

    // Working bodies breathe; the alert overrides that beat with its own.
    const breath = status === 'working' && !reduced ? 1 + Math.sin(t * 2.2) * 0.05 : 1;
    const heat = (alerting ? 0.4 + blink * 0.85 : 0.34 + s.live * 0.72) * (1 + s.lift * 0.3);

    outerMat.current.color.copy(tint);
    innerMat.current.color.copy(tint);
    outerMat.current.opacity = Math.min(1, 0.5 * heat + 0.16);
    innerMat.current.opacity = Math.min(1, 0.34 * heat + 0.08);
    haloMat.current.color.copy(tint);
    haloMat.current.opacity = Math.min(1, 0.2 + heat * 0.5);
    halo.current.scale.setScalar(4.4 * breath * (1 + s.lift * 0.08));
    body.current.scale.setScalar(breath * (1 + s.lift * 0.08));
  });

  const radius = hudGlobeRadius(fillPct);

  return (
    <group scale={radius}>
      <sprite ref={halo} scale={4.4} renderOrder={-1}>
        <spriteMaterial
          ref={haloMat}
          map={glow}
          color={base}
          transparent
          opacity={0.4}
          depthWrite={false}
          blending={THREE.AdditiveBlending}
          toneMapped={false}
        />
      </sprite>
      <group ref={body}>
        <lineSegments geometry={outerGeo}>
          <lineBasicMaterial
            ref={outerMat}
            color={base}
            transparent
            opacity={0.6}
            depthWrite={false}
            blending={THREE.AdditiveBlending}
            toneMapped={false}
          />
        </lineSegments>
        <lineSegments geometry={innerGeo}>
          <lineBasicMaterial
            ref={innerMat}
            color={base}
            transparent
            opacity={0.35}
            depthWrite={false}
            blending={THREE.AdditiveBlending}
            toneMapped={false}
          />
        </lineSegments>
        <AgentCore
          harness={harness}
          color={base}
          dimmed={status === 'idle'}
          active={hovered}
          working={status === 'working'}
        />
      </group>
    </group>
  );
}
