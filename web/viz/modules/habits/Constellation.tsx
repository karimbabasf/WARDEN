// Constellation.tsx — the agent-grouping cue, drawn as a star chart.
//
// This retires the flat camera-facing TerritoryRing (a childish hoop around each
// cluster) in favour of a constellation drawn in light:
//
//   • tethers   a curved, bowed cable from every habit back to its agent core,
//               gradient-lit (dim harness tint at the core → the habit's own
//               severity colour at the far end) with an energy mote flowing
//               inward. These ARE the membership signal — one tether per real
//               habit→agent link, nothing decorative.
//   • figure    faint "connect-the-dots" lines between each habit and its two
//               nearest neighbours on the shell — the constellation outline that
//               traces the cluster's shape, the way a star chart joins stars.
//
// Honest-viz: every tether maps to a layout link; the figure only ever joins a
// cluster's own habits. Curves give the web genuine 3D volume so it reads as a
// body, not a hoop.

import { useEffect, useMemo, useRef } from 'react';
import { useFrame } from '@react-three/fiber';
import * as THREE from 'three';
import { harnessTheme, severityColor } from '@/viz/shared/theme/harnessTheme';
import type { LayoutNode, OrbLayout } from '@/viz/shared/types/orbTypes';

const SEG = 20; // samples per tether curve
const UP = new THREE.Vector3(0, 1, 0);

// soft round sprite for the flowing energy motes
let dotCache: THREE.Texture | null = null;
function dotTexture(): THREE.Texture {
  if (dotCache) return dotCache;
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
  dotCache = new THREE.CanvasTexture(c);
  dotCache.needsUpdate = true;
  return dotCache;
}

function nodeDist(a: LayoutNode, b: LayoutNode): number {
  return Math.hypot(a.position.x - b.position.x, a.position.y - b.position.y, a.position.z - b.position.z);
}

// Minimum spanning tree (Prim's) over a set of nodes — the sparsest set of links
// (N-1 edges) that still joins every star into one shape. A k-nearest graph over
// an evenly-spread shell collapses into a dense geodesic globe; an MST stays a
// delicate branching figure, the way a star chart joins a constellation.
function mstEdges(nodes: LayoutNode[]): Array<[number, number]> {
  const n = nodes.length;
  if (n < 2) return [];
  const inTree = new Array<boolean>(n).fill(false);
  const best = new Array<number>(n).fill(Infinity);
  const parent = new Array<number>(n).fill(-1);
  best[0] = 0;
  const edges: Array<[number, number]> = [];
  for (let it = 0; it < n; it++) {
    let u = -1;
    let bd = Infinity;
    for (let v = 0; v < n; v++) if (!inTree[v] && best[v] < bd) { bd = best[v]; u = v; }
    if (u === -1) break;
    inTree[u] = true;
    if (parent[u] !== -1) edges.push([parent[u], u]);
    for (let v = 0; v < n; v++) {
      if (inTree[v]) continue;
      const d = nodeDist(nodes[u], nodes[v]);
      if (d < best[v]) {
        best[v] = d;
        parent[v] = u;
      }
    }
  }
  return edges;
}

export function ConstellationWeb({ layout }: { layout: OrbLayout }) {
  const byId = useMemo(() => new Map(layout.nodes.map((n) => [n.id, n])), [layout]);
  const links = useMemo(
    () => layout.links.filter((l) => byId.has(l.source) && byId.has(l.target)),
    [layout, byId],
  );

  // Curved tethers: one bowed bezier per link, sampled into a gradient polyline.
  const { tetherGeo, curves, dotGeo, phases } = useMemo(() => {
    const curves: THREE.QuadraticBezierCurve3[] = [];
    const linePos: number[] = [];
    const lineCol: number[] = [];
    const dotPos: number[] = [];
    const dotCol: number[] = [];
    const phases: number[] = [];

    links.forEach((link, idx) => {
      const hub = byId.get(link.source)!;
      const iss = byId.get(link.target)!;
      const h = new THREE.Vector3(hub.position.x, hub.position.y, hub.position.z);
      const i = new THREE.Vector3(iss.position.x, iss.position.y, iss.position.z);
      const dir = new THREE.Vector3().subVectors(i, h);
      const len = dir.length() || 1;
      const mid = new THREE.Vector3().addVectors(h, i).multiplyScalar(0.5);

      // bow the cable off the straight line (perpendicular + a little lift) so the
      // web has real depth instead of collapsing onto flat spokes.
      const perp = new THREE.Vector3().crossVectors(dir, UP);
      if (perp.lengthSq() < 1e-4) perp.set(1, 0, 0);
      perp.normalize();
      const ctrl = mid
        .clone()
        .add(perp.multiplyScalar(len * 0.16))
        .add(UP.clone().multiplyScalar(len * 0.06));
      const curve = new THREE.QuadraticBezierCurve3(h, ctrl, i);
      curves.push(curve);
      phases.push((idx * 0.37) % 1);

      const cHub = new THREE.Color(harnessTheme(hub.harness).color);
      const cIss = new THREE.Color(
        iss.issue ? severityColor(iss.issue.severity) : harnessTheme(iss.harness).color,
      );
      const pts = curve.getPoints(SEG);
      for (let s = 0; s < SEG; s++) {
        const ta = s / SEG;
        const tb = (s + 1) / SEG;
        const a = pts[s];
        const b = pts[s + 1];
        // colour blends hub→habit; brightness fades toward the core so it doesn't
        // blow out where every tether converges.
        const ca = cHub.clone().lerp(cIss, ta).multiplyScalar(0.35 + 0.65 * ta);
        const cb = cHub.clone().lerp(cIss, tb).multiplyScalar(0.35 + 0.65 * tb);
        linePos.push(a.x, a.y, a.z, b.x, b.y, b.z);
        lineCol.push(ca.r, ca.g, ca.b, cb.r, cb.g, cb.b);
      }
      dotPos.push(i.x, i.y, i.z);
      dotCol.push(cIss.r, cIss.g, cIss.b);
    });

    const tetherGeo = new THREE.BufferGeometry();
    tetherGeo.setAttribute('position', new THREE.Float32BufferAttribute(linePos, 3));
    tetherGeo.setAttribute('color', new THREE.Float32BufferAttribute(lineCol, 3));
    const dotGeo = new THREE.BufferGeometry();
    dotGeo.setAttribute('position', new THREE.Float32BufferAttribute(dotPos, 3));
    dotGeo.setAttribute('color', new THREE.Float32BufferAttribute(dotCol, 3));
    return { tetherGeo, curves, dotGeo, phases };
  }, [links, byId]);

  // Constellation figure: a minimum spanning tree over each agent's habits — the
  // sparsest set of links that still joins every star into one drawn shape.
  const webGeo = useMemo(() => {
    const byAgent = new Map<string, LayoutNode[]>();
    for (const n of layout.nodes) {
      if (n.kind !== 'issue') continue;
      if (!byAgent.has(n.agentId)) byAgent.set(n.agentId, []);
      byAgent.get(n.agentId)!.push(n);
    }
    const pos: number[] = [];
    const col: number[] = [];
    for (const [, issues] of byAgent) {
      if (issues.length < 3) continue; // a figure needs a few stars
      const c = new THREE.Color(harnessTheme(issues[0].harness).color);
      for (const [a, b] of mstEdges(issues)) {
        const da = issues[a];
        const db = issues[b];
        pos.push(da.position.x, da.position.y, da.position.z, db.position.x, db.position.y, db.position.z);
        col.push(c.r, c.g, c.b, c.r, c.g, c.b);
      }
    }
    const g = new THREE.BufferGeometry();
    g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
    g.setAttribute('color', new THREE.Float32BufferAttribute(col, 3));
    return g;
  }, [layout]);

  const tetherMat = useRef<THREE.LineBasicMaterial>(null);
  const webMat = useRef<THREE.LineBasicMaterial>(null);
  const dotTex = useMemo(() => dotTexture(), []);
  const tmp = useMemo(() => new THREE.Vector3(), []);

  useEffect(
    () => () => {
      tetherGeo.dispose();
      dotGeo.dispose();
      webGeo.dispose();
    },
    [tetherGeo, dotGeo, webGeo],
  );

  useFrame((state) => {
    const t = state.clock.elapsedTime;
    const attr = dotGeo.getAttribute('position') as THREE.BufferAttribute;
    for (let i = 0; i < curves.length; i++) {
      const tt = (t * 0.3 + phases[i]) % 1;
      curves[i].getPoint(1 - tt, tmp); // mote flows habit → core
      attr.setXYZ(i, tmp.x, tmp.y, tmp.z);
    }
    attr.needsUpdate = true;
    if (tetherMat.current) tetherMat.current.opacity = 0.34 + Math.sin(t * 1.3) * 0.06;
    if (webMat.current) webMat.current.opacity = 0.11 + Math.sin(t * 0.8 + 1.4) * 0.045;
  });

  if (links.length === 0) return null;
  return (
    <group>
      <lineSegments geometry={webGeo}>
        <lineBasicMaterial
          ref={webMat}
          vertexColors
          transparent
          opacity={0.11}
          depthWrite={false}
          blending={THREE.AdditiveBlending}
          toneMapped={false}
        />
      </lineSegments>
      <lineSegments geometry={tetherGeo}>
        <lineBasicMaterial
          ref={tetherMat}
          vertexColors
          transparent
          opacity={0.36}
          depthWrite={false}
          blending={THREE.AdditiveBlending}
          toneMapped={false}
        />
      </lineSegments>
      <points geometry={dotGeo}>
        <pointsMaterial
          vertexColors
          size={0.16}
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

export default ConstellationWeb;
