// pageRenderer.ts: draws instruction-book pages from the LDraw steps themselves.
//
// A page is what a real manual shows for one step: a big step number, a callout with
// the parts the step adds (all drawn at one scale, so a 1x1 plate looks small next to
// a 2x4 brick), and the model as it stands after the step, framed on what is built so
// far. Pages render on demand (a page turn, not a frame) through ONE small offscreen
// WebGL renderer shared by every bench, are cached, and are requested through
// `requestPage`, which drops pages a flurry has already turned past.

import * as THREE from 'three';
import { collectBricks, loadTemplate } from './setModel';

/** Logical page size; the canvas is drawn at SCALE for a crisp page up close. */
export const PAGE_W = 360;
export const PAGE_H = 480;
const SCALE = 2;
const VIEW = 300;
const PART_CELL = 78;

const PAPER_TOP = '#fbfcfe';
const PAPER_BOTTOM = '#eef2f7';
const INK = '#141b26';
const SOFT_INK = '#5d6f86';
const CALLOUT_FILL = '#d6e6f5';
const CALLOUT_RULE = '#9dbbd8';

type SetScene = {
  model: THREE.Group;
  bricks: THREE.Object3D[];
  stepOf: number[];
  centre: THREE.Vector3;
  radius: number;
};

let renderer: THREE.WebGLRenderer | null = null;
let scene: THREE.Scene | null = null;
let camera: THREE.PerspectiveCamera | null = null;

function ensureRenderer(): { r: THREE.WebGLRenderer; s: THREE.Scene; c: THREE.PerspectiveCamera } {
  if (!renderer || !scene || !camera) {
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, preserveDrawingBuffer: true });
    renderer.setPixelRatio(1);
    renderer.setSize(VIEW * SCALE, VIEW * SCALE, false);
    renderer.toneMapping = THREE.ACESFilmicToneMapping;
    renderer.toneMappingExposure = 1.2;
    scene = new THREE.Scene();
    // Manual lighting: bright and even, a key from the upper left, no hard shadows.
    scene.add(new THREE.HemisphereLight(0xffffff, 0x9aa8b8, 1.9));
    const key = new THREE.DirectionalLight(0xffffff, 1.9);
    key.position.set(-3, 6, 4);
    scene.add(key);
    const fill = new THREE.DirectionalLight(0xdfe9ff, 0.7);
    fill.position.set(4, 1, 3);
    scene.add(fill);
    camera = new THREE.PerspectiveCamera(26, 1, 0.01, 100);
  }
  return { r: renderer, s: scene, c: camera };
}

const setScenes = new Map<string, Promise<SetScene>>();

function setScene(file: string): Promise<SetScene> {
  let p = setScenes.get(file);
  if (!p) {
    p = loadTemplate(file).then((t) => {
      const model = t.clone(true) as THREE.Group;
      model.rotation.set(Math.PI, 0, 0);
      model.updateMatrixWorld(true);
      const bricks = collectBricks(model);
      const box = new THREE.Box3().setFromObject(model);
      const sphere = box.getBoundingSphere(new THREE.Sphere());
      return {
        model,
        bricks,
        stepOf: bricks.map((b) => (b.userData.buildingStep as number | undefined) ?? 0),
        centre: sphere.center,
        radius: sphere.radius,
      };
    });
    setScenes.set(file, p);
  }
  return p;
}

/** The classic manual angle: from the front-right, a little above. */
function aim(c: THREE.PerspectiveCamera, centre: THREE.Vector3, radius: number) {
  const dist = (radius / Math.sin(THREE.MathUtils.degToRad(c.fov / 2))) * 0.95;
  const dir = new THREE.Vector3(0.62, 0.58, 0.82).normalize();
  c.position.copy(centre).addScaledVector(dir, dist);
  c.near = dist / 50;
  c.far = dist * 4;
  c.lookAt(centre);
  c.updateProjectionMatrix();
}

function snapshot(g: CanvasRenderingContext2D, x: number, y: number, size: number) {
  g.drawImage(ensureRenderer().r.domElement, x, y, size, size);
}

function roundRect(g: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, rad: number) {
  g.beginPath();
  g.moveTo(x + rad, y);
  g.arcTo(x + w, y, x + w, y + h, rad);
  g.arcTo(x + w, y + h, x, y + h, rad);
  g.arcTo(x, y + h, x, y, rad);
  g.arcTo(x, y, x + w, y, rad);
  g.closePath();
}

/** The page's fonts come from the app's own @font-face rules. */
const DISPLAY = '"Technor", system-ui, sans-serif';
const MONO = '"CommitMono", ui-monospace, monospace';

const pageCache = new Map<string, HTMLCanvasElement>();
const PAGE_CACHE_MAX = 96;

async function draw(file: string, step: number, steps: number, label: string): Promise<HTMLCanvasElement> {
  const ss = await setScene(file);
  const { r, s, c } = ensureRenderer();
  const canvas = document.createElement('canvas');
  canvas.width = PAGE_W * SCALE;
  canvas.height = PAGE_H * SCALE;
  const g = canvas.getContext('2d') as CanvasRenderingContext2D;
  g.scale(SCALE, SCALE);
  const paper = g.createLinearGradient(0, 0, 0, PAGE_H);
  paper.addColorStop(0, PAPER_TOP);
  paper.addColorStop(1, PAPER_BOTTOM);
  g.fillStyle = paper;
  g.fillRect(0, 0, PAGE_W, PAGE_H);

  const finished = step >= steps;
  s.add(ss.model);
  try {
    // The parts this step adds, grouped by part and colour.
    const kinds = new Map<string, { b: THREE.Object3D; n: number }>();
    if (!finished) {
      ss.bricks.forEach((b, i) => {
        if (ss.stepOf[i] !== step) return;
        const k = `${b.userData.fileName}|${b.userData.colorCode}`;
        const cur = kinds.get(k);
        if (cur) cur.n++;
        else kinds.set(k, { b, n: 1 });
      });
    }
    // Step number, top left, the first thing the eye lands on.
    g.fillStyle = INK;
    g.textBaseline = 'top';
    g.font = `700 72px ${DISPLAY}`;
    const numW = finished ? 0 : g.measureText(String(step + 1)).width;
    if (!finished) g.fillText(String(step + 1), 18, 12);
    // The callout starts after the number and takes as many parts as fit the page.
    const boxX = Math.round(18 + numW + 14);
    const fit = Math.max(1, Math.floor((PAGE_W - 16 - boxX - 18) / PART_CELL));
    const shown = [...kinds.values()].slice(0, fit);

    if (shown.length > 0) {
      // One scale for every part in the callout, set by the biggest.
      const spheres = shown.map(({ b }) => {
        b.updateMatrixWorld(true);
        return new THREE.Box3().setFromObject(b).getBoundingSphere(new THREE.Sphere());
      });
      const maxR = Math.max(...spheres.map((sp) => sp.radius), 1e-3);
      const boxW = 18 + shown.length * PART_CELL;
      g.fillStyle = CALLOUT_FILL;
      g.strokeStyle = CALLOUT_RULE;
      g.lineWidth = 1.5;
      roundRect(g, boxX, 18, boxW, PART_CELL + 30, 9);
      g.fill();
      g.stroke();
      shown.forEach(({ b, n }, i) => {
        ss.bricks.forEach((x) => {
          x.visible = x === b;
        });
        aim(c, spheres[i].center, maxR);
        r.setClearColor(0x000000, 0);
        r.render(s, c);
        const x = boxX + 9 + i * PART_CELL;
        snapshot(g, x, 22, PART_CELL);
        g.fillStyle = INK;
        g.font = `700 16px ${MONO}`;
        g.textBaseline = 'alphabetic';
        g.fillText(`${n}x`, x + 6, 18 + PART_CELL + 22);
      });
    }

    // The model after this step, framed on what is built so far (the way a manual
    // zooms in while a model is small), never tighter than a third of the whole.
    const built = new THREE.Box3();
    ss.bricks.forEach((b, i) => {
      b.visible = ss.stepOf[i] <= step;
      if (b.visible) built.expandByObject(b);
    });
    const sph = built.isEmpty() ? new THREE.Sphere(ss.centre, ss.radius) : built.getBoundingSphere(new THREE.Sphere());
    aim(c, sph.center, Math.max(sph.radius, ss.radius / 3));
    r.setClearColor(0x000000, 0);
    r.render(s, c);
    const top = 18 + PART_CELL + 36;
    const size = Math.min(PAGE_W - 24, PAGE_H - top - 34);
    // A soft floor shadow grounds the model on the page.
    const sx = PAGE_W / 2;
    const sy = top + size * 0.86;
    const shadow = g.createRadialGradient(sx, sy, 2, sx, sy, size * 0.42);
    shadow.addColorStop(0, 'rgba(40, 60, 90, 0.18)');
    shadow.addColorStop(1, 'rgba(40, 60, 90, 0)');
    g.save();
    g.translate(sx, sy);
    g.scale(1, 0.28);
    g.translate(-sx, -sy);
    g.fillStyle = shadow;
    g.fillRect(0, sy - size * 0.42, PAGE_W, size * 0.84);
    g.restore();
    snapshot(g, (PAGE_W - size) / 2, top, size);

    // Footer: the set on the left, the page tab on the right, a thin progress rule.
    g.textBaseline = 'alphabetic';
    g.fillStyle = SOFT_INK;
    g.font = `600 12px ${MONO}`;
    g.fillText(label, 20, PAGE_H - 16);
    const prog = Math.min(1, (step + 1) / Math.max(1, steps));
    g.fillStyle = 'rgba(93, 111, 134, 0.18)';
    g.fillRect(20, PAGE_H - 36, PAGE_W - 40, 4);
    g.fillStyle = SOFT_INK;
    g.fillRect(20, PAGE_H - 36, (PAGE_W - 40) * prog, 4);
    const tab = finished ? 'done' : `${step + 1}/${steps}`;
    g.font = `600 11px ${MONO}`;
    const tw = g.measureText(tab).width + 14;
    g.fillStyle = INK;
    roundRect(g, PAGE_W - 20 - tw, PAGE_H - 28, tw, 18, 4);
    g.fill();
    g.fillStyle = '#ffffff';
    g.fillText(tab, PAGE_W - 20 - tw + 7, PAGE_H - 15);
    if (finished) {
      g.fillStyle = INK;
      g.font = `700 26px ${DISPLAY}`;
      g.textBaseline = 'top';
      g.fillText('Finished', 20, 26);
    }
  } finally {
    s.remove(ss.model);
  }
  return canvas;
}

/** The page for `step` of `file`. `label` is the set line in the footer. */
export async function renderPage(file: string, step: number, steps: number, label = ''): Promise<HTMLCanvasElement> {
  const key = `${file}#${step}`;
  const hit = pageCache.get(key);
  if (hit) return hit;
  const canvas = await draw(file, step, steps, label);
  pageCache.set(key, canvas);
  if (pageCache.size > PAGE_CACHE_MAX) {
    const oldest = pageCache.keys().next().value;
    if (oldest !== undefined) pageCache.delete(oldest);
  }
  return canvas;
}

/**
 * Ask for a page, coalesced per bench: while a flurry turns pages faster than they
 * render, only the newest request is drawn, and the GPU work waits for an idle moment
 * so it never lands in the middle of a frame. Resolves null for a superseded request.
 */
const latest = new Map<string, number>();
export function requestPage(
  benchId: string,
  file: string,
  step: number,
  steps: number,
  label: string,
): Promise<HTMLCanvasElement | null> {
  const ticket = (latest.get(benchId) ?? 0) + 1;
  latest.set(benchId, ticket);
  return new Promise((resolve) => {
    const go = () => {
      if (latest.get(benchId) !== ticket) return resolve(null);
      renderPage(file, step, steps, label).then(resolve, () => resolve(null));
    };
    const idle = (window as unknown as { requestIdleCallback?: (cb: () => void, o?: { timeout: number }) => void })
      .requestIdleCallback;
    if (idle) idle(go, { timeout: 250 });
    else window.setTimeout(go, 60);
  });
}
