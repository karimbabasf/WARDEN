import { describe, it, expect } from 'vitest';
import { reduce, createBridge, type SceneState } from './bridge';

// `createBridge` takes the Tauri `listen` so the bridge can self-wire in the app, but
// every test drives it synchronously through `ingest(name, payload)`, no Tauri runtime
// required. We pass a no-op listen stub.
const noopListen = (async () => () => {}) as unknown as Parameters<typeof createBridge>[0];

function snapshot(bridge: ReturnType<typeof createBridge>): SceneState {
  let latest!: SceneState;
  const unsub = bridge.subscribe((s) => {
    latest = s;
  });
  unsub();
  return latest;
}

const empty = (): SceneState => ({ minimized: false });

describe('reduce: radar_scene_ready', () => {
  it('normalizes the live forest into radarScene', () => {
    const next = reduce(empty(), 'radar_scene_ready', {
      generatedAt: '2026-07-10T00:00:00Z',
      agents: [{ id: 'a1', harness: 'codex', label: 'Codex' }],
    });
    expect(next.radarScene?.agents.length).toBe(1);
    expect(next.radarScene?.agents[0]?.id).toBe('a1');
  });

  it('never throws on a malformed payload (schema drift stays inert)', () => {
    const next = reduce(empty(), 'radar_scene_ready', null);
    expect(next.radarScene?.agents).toEqual([]);
  });
});

describe('reduce: window lifecycle', () => {
  it('warden_hotkey summons and clears any minimize pause', () => {
    const next = reduce({ minimized: true }, 'warden_hotkey', {});
    expect(next.summoned).toBe(true);
    expect(next.minimized).toBe(false);
  });

  it('warden_hotkey is a no-op once summoned and not minimized', () => {
    const s: SceneState = { summoned: true, minimized: false };
    expect(reduce(s, 'warden_hotkey', {})).toBe(s);
  });

  it('minimize gates the render loop and restore ungates it', () => {
    const min = reduce(empty(), 'warden_minimized', {});
    expect(min.minimized).toBe(true);
    expect(reduce(min, 'warden_restored', {}).minimized).toBe(false);
  });

  it('warden_dismiss drops the summon flag', () => {
    expect(reduce({ summoned: true }, 'warden_dismiss', {}).summoned).toBe(false);
  });
});

describe('reduce: non scene-driving events are inert', () => {
  it('returns the same reference for events the radar does not consume', () => {
    const s = empty();
    expect(reduce(s, 'ingest_progress', { phase: 'live' })).toBe(s);
    expect(reduce(s, 'totally_unknown', {})).toBe(s);
  });
});

describe('createBridge', () => {
  it('pushes the current snapshot on subscribe and on each accepted event', () => {
    const bridge = createBridge(noopListen);
    const seen: SceneState[] = [];
    bridge.subscribe((s) => seen.push(s));
    expect(seen.length).toBe(1); // immediate snapshot
    bridge.ingest('radar_scene_ready', { agents: [{ id: 'a1', harness: 'codex' }], generatedAt: '' });
    expect(seen.length).toBe(2);
    expect(seen[1]?.radarScene?.agents.length).toBe(1);
  });

  it('reset preserves the radar forest and the window state', () => {
    const bridge = createBridge(noopListen);
    bridge.ingest('radar_scene_ready', { agents: [{ id: 'a1', harness: 'codex' }], generatedAt: '' });
    bridge.ingest('warden_hotkey', {});
    bridge.reset();
    const s = snapshot(bridge);
    expect(s.radarScene?.agents.length).toBe(1);
    expect(s.summoned).toBe(true);
  });
});
