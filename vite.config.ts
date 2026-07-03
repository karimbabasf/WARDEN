/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  clearScreen: false,
  plugins: [react()],
  server: {
    // HARD-PINNED to 1421. Tauri's webview devUrl (tauri.conf.json) points here,
    // so the two MUST agree. 1420 is deliberately left to the other Tauri apps on
    // this machine (e.g. the Trading app) that squat the Tauri-default 1420.
    // strictPort: true means if 1421 is taken, Vite CRASHES loudly instead of
    // sliding to the next free port — because a silent slide would strand the
    // webview on whatever else is on 1421. That silent slide is the exact bug
    // this pin fixes (WARDEN's window was loading the Trading app off 1420).
    // The full-app launcher (scripts/warden-dev.mjs) still overrides BOTH sides
    // in lockstep via WARDEN_DEV_PORT when you need a different free port.
    port: Number(process.env.WARDEN_DEV_PORT) || 1421,
    strictPort: true,
    host: '127.0.0.1'
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: 'es2022',
    minify: 'esbuild',
    sourcemap: true
  },
  test: {
    // The bridge + layout cores are pure logic; node is enough and keeps that
    // suite fast (3D render output is verified live, never asserted here).
    // Phase-3 panel/card COMPONENT tests render real DOM, so they live in
    // `*.test.tsx` files that each opt into jsdom via a `// @vitest-environment
    // jsdom` pragma (same pattern as mount.test.ts). The default env stays node;
    // broadening `include` to also match `.test.tsx` is all that is needed.
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx']
  }
});
