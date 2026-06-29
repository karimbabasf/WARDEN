#!/usr/bin/env node
// WARDEN full-app dev launcher.
//
// Picks a free TCP port (preferring 1420, falling forward if it's taken),
// then starts `tauri dev` with BOTH sides pointed at that same port:
//   - Vite reads WARDEN_DEV_PORT (see vite.config.ts) and binds it strictly.
//   - Tauri's webview devUrl is overridden via --config to match.
// This is why running a second instance — or having anything else on 1420 —
// no longer crashes the dev server: it just slides to the next open port.
import { createServer } from 'node:net';
import { spawn } from 'node:child_process';

const BASE_PORT = Number(process.env.WARDEN_DEV_PORT) || 1420;
const SCAN = 100; // ports to probe before giving up

function isFree(port) {
  return new Promise((resolve) => {
    const srv = createServer();
    srv.once('error', () => resolve(false));
    srv.once('listening', () => srv.close(() => resolve(true)));
    srv.listen(port, '127.0.0.1');
  });
}

async function pickPort(start) {
  for (let p = start; p < start + SCAN; p += 1) {
    // eslint-disable-next-line no-await-in-loop
    if (await isFree(p)) return p;
  }
  throw new Error(`no free port in [${start}, ${start + SCAN})`);
}

const port = await pickPort(BASE_PORT);
if (port !== BASE_PORT) {
  console.log(`[warden-dev] port ${BASE_PORT} busy → using ${port}`);
}

const devUrl = `http://127.0.0.1:${port}`;
const child = spawn(
  'pnpm',
  ['exec', 'tauri', 'dev', '--config', JSON.stringify({ build: { devUrl } })],
  { stdio: 'inherit', env: { ...process.env, WARDEN_DEV_PORT: String(port) } },
);

child.on('exit', (code) => process.exit(code ?? 0));
process.on('SIGINT', () => child.kill('SIGINT'));
process.on('SIGTERM', () => child.kill('SIGTERM'));
