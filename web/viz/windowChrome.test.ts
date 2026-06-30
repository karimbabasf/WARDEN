/// <reference types="vite/client" />

import { describe, expect, it } from 'vitest';
import tauriConfigRaw from '../../src-tauri/tauri.conf.json?raw';
import stylesSource from '../style.css?raw';
import navBarSource from '@/viz/views/war-room/NavBar.tsx?raw';
import warRoomSource from '@/viz/views/war-room/WarRoom.tsx?raw';

function appWindow(): Record<string, unknown> {
  const config = JSON.parse(tauriConfigRaw);
  return config.app.windows[0];
}

describe('macOS window chrome', () => {
  it('uses a normal decorated title bar for instant native drag and zoom', () => {
    const win = appWindow();

    expect(win.decorations).toBe(true);
    expect(win.titleBarStyle ?? 'Visible').toBe('Visible');
    expect(win.hiddenTitle ?? false).toBe(false);
    expect(win.resizable).toBe(true);
    expect(win.maximizable).toBe(true);
    expect(win.visibleOnAllWorkspaces ?? false).toBe(false);
  });

  it('does not route window movement through webview drag regions', () => {
    expect(warRoomSource).not.toContain('data-tauri-drag-region');
    expect(navBarSource).not.toContain('data-tauri-drag-region');
    expect(stylesSource).not.toContain('.wd-dragbar');
  });
});
