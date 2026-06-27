/// <reference types="vitest/config" />
import { fileURLToPath, URL } from 'node:url';
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  clearScreen: false,
  plugins: [react()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url))
    }
  },
  server: {
    port: 1420,
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
