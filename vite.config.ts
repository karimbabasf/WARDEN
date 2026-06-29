/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  clearScreen: false,
  plugins: [react()],
  server: {
    // Default to 1420, but never hard-pin to it. Plain `pnpm dev` is
    // non-strict, so if 1420 is taken Vite falls forward to the next free
    // port instead of crashing. The full-app launcher (scripts/warden-dev.mjs)
    // pre-picks a free port and exports WARDEN_DEV_PORT; in that case we bind
    // it strictly so Vite and Tauri's (overridden) devUrl can never disagree.
    port: Number(process.env.WARDEN_DEV_PORT) || 1420,
    strictPort: Boolean(process.env.WARDEN_DEV_PORT),
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
