// hudGlow.ts: the soft halo behind a HUD globe, as a canvas-drawn radial sprite.
//
// The war-room constellation gets its bloom from a real postprocessing pass. The HUD
// cannot: its canvas is TRANSPARENT so the panel's own material shows through, and an
// EffectComposer render target has no alpha to give back. A single additive sprite per
// globe buys the same read (a lit body rather than a wireframe drawing) at a fraction
// of the cost, on a surface that is on screen for two seconds at a time.
//
// One texture, built once and shared by every globe.

import * as THREE from 'three';

let cached: THREE.Texture | null = null;

/** A 128px radial falloff, white, alpha-only. Tinted per globe by the sprite material. */
export function hudGlowTexture(): THREE.Texture {
  if (cached) return cached;
  const size = 128;
  const canvas = document.createElement('canvas');
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext('2d');
  if (!ctx) {
    cached = new THREE.Texture();
    return cached;
  }
  const g = ctx.createRadialGradient(size / 2, size / 2, 0, size / 2, size / 2, size / 2);
  // Steep in the middle, long tail: a linear ramp reads as a flat disc, and a ramp
  // that ends above zero paints a visible square edge on a transparent canvas.
  g.addColorStop(0, 'rgba(255,255,255,0.95)');
  g.addColorStop(0.22, 'rgba(255,255,255,0.42)');
  g.addColorStop(0.55, 'rgba(255,255,255,0.10)');
  g.addColorStop(1, 'rgba(255,255,255,0)');
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, size, size);
  const tex = new THREE.CanvasTexture(canvas);
  tex.colorSpace = THREE.SRGBColorSpace;
  cached = tex;
  return tex;
}
