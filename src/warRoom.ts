import * as THREE from 'three';

export type UsagePulse = { input?: number; output?: number; orchestrationInput?: number; orchestrationOutput?: number; stage?: string };

export class WarRoom {
  private renderer: THREE.WebGLRenderer;
  private scene: THREE.Scene;
  private camera: THREE.PerspectiveCamera;
  private nodes: THREE.Mesh[] = [];
  private intensity = 0.18;
  private stage = 'idle';

  constructor(private canvas: HTMLCanvasElement) {
    this.renderer = new THREE.WebGLRenderer({ canvas, alpha: true, antialias: true });
    this.scene = new THREE.Scene();
    this.camera = new THREE.PerspectiveCamera(48, 1, 0.1, 100);
    this.camera.position.z = 8;
    const geo = new THREE.IcosahedronGeometry(0.32, 1);
    for (let i = 0; i < 3; i++) {
      const mat = new THREE.MeshBasicMaterial({ color: i === 0 ? 0x76ff9d : 0x1b6f3a, wireframe: true, transparent: true, opacity: 0.82 });
      const m = new THREE.Mesh(geo, mat);
      const a = (i / 3) * Math.PI * 2;
      m.position.set(Math.cos(a) * 2.1, Math.sin(a) * 1.2, 0);
      this.scene.add(m);
      this.nodes.push(m);
    }
    const ring = new THREE.Mesh(new THREE.TorusGeometry(2.6, 0.006, 8, 160), new THREE.MeshBasicMaterial({ color: 0x76ff9d, transparent: true, opacity: 0.18 }));
    this.scene.add(ring);
    window.addEventListener('resize', () => this.resize());
    this.resize();
    this.tick();
  }

  pulse(p: UsagePulse) {
    const orch = (p.orchestrationInput ?? 0) + (p.orchestrationOutput ?? 0);
    const regular = (p.input ?? 0) + (p.output ?? 0);
    this.intensity = Math.min(2.4, 0.32 + Math.log10(orch + regular + 10) / 2.5);
    if (p.stage) this.stage = p.stage;
    const active = this.stage.includes('verify') ? 1 : this.stage.includes('coach') ? 2 : 0;
    this.nodes.forEach((n, i) => {
      const mat = n.material as THREE.MeshBasicMaterial;
      mat.color.setHex(i === active ? 0xb8ff6b : 0x76ff9d);
      mat.opacity = i === active ? 0.98 : 0.42;
    });
  }

  private resize() {
    const w = window.innerWidth, h = window.innerHeight;
    this.renderer.setSize(w, h, false);
    this.camera.aspect = w / h;
    this.camera.updateProjectionMatrix();
  }

  private tick = () => {
    const t = performance.now() / 1000;
    this.nodes.forEach((n, i) => {
      n.rotation.x += 0.004 + this.intensity * 0.001;
      n.rotation.y += 0.007 + this.intensity * 0.001;
      const s = 1 + Math.sin(t * 2.2 + i) * 0.05 + this.intensity * 0.08;
      n.scale.setScalar(s);
    });
    this.renderer.render(this.scene, this.camera);
    requestAnimationFrame(this.tick);
  };
}
