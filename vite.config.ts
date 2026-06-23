/// <reference types="vitest/config" />
import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

export default defineConfig({
  clearScreen: false,
  plugins: [react()],
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
    // The bridge is pure logic; node is enough and keeps the unit suite fast.
    // (3D render output is verified live, never asserted here.)
    environment: 'node',
    include: ['src/**/*.test.ts']
  }
});
