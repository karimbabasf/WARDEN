// @vitest-environment jsdom
//
// PeersPanel is the observer side: add a token, watch the peer list, select one to see
// its live (redacted) radar, remove one. The three load-bearing flows are add, remove and
// switch, each verified against the exact command/args the frozen contract defines.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { PeersPanel } from './PeersPanel';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

let container: HTMLDivElement | null = null;
let root: Root | null = null;

function render(node: React.ReactNode): HTMLElement {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(node));
  return container;
}

async function flush() {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

const PEER = {
  peerId: 'peer-1',
  hostLabel: "Askhat's machine",
  fingerprint: 'AB12:CD34:EF56:0000',
  connected: true,
  lastFrameAt: '2026-07-26T12:00:00Z',
  error: null,
};

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockReturnValue(Promise.resolve(() => {}));
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
});

async function open(el: HTMLElement) {
  const trigger = el.querySelector<HTMLButtonElement>('.wd-observe-watch-trigger');
  act(() => trigger!.click());
  await flush();
}

describe('PeersPanel', () => {
  it('shows an honest empty state before any peer is added', async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => (cmd === 'observe_list_peers' ? Promise.resolve([]) : Promise.resolve(undefined)));
    const el = render(<PeersPanel />);
    await open(el);
    expect(el.textContent).toContain('Not watching anyone yet');
  });

  it('adding a token calls observe_add_peer with the trimmed token and refreshes the list', async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'observe_list_peers') return Promise.resolve([]);
      if (cmd === 'observe_add_peer') return Promise.resolve(PEER);
      return Promise.resolve(undefined);
    });
    const el = render(<PeersPanel />);
    await open(el);

    const input = el.querySelector<HTMLInputElement>('.wd-observe-token-input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
    act(() => {
      setter?.call(input, '  a-friends-token  ');
      input!.dispatchEvent(new Event('input', { bubbles: true }));
    });

    const addBtn = Array.from(el.querySelectorAll('button')).find((b) => b.textContent === 'Add') as HTMLButtonElement;
    act(() => addBtn.click());
    await flush();

    expect(invoke).toHaveBeenCalledWith('observe_add_peer', { token: 'a-friends-token' });
    expect(invoke).toHaveBeenCalledWith('observe_list_peers');
  });

  it('surfaces an add failure (e.g. an expired token) without crashing', async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'observe_list_peers') return Promise.resolve([]);
      if (cmd === 'observe_add_peer') return Promise.reject('token expired');
      return Promise.resolve(undefined);
    });
    const el = render(<PeersPanel />);
    await open(el);
    const input = el.querySelector<HTMLInputElement>('.wd-observe-token-input');
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
    act(() => {
      setter?.call(input, 'bad-token');
      input!.dispatchEvent(new Event('input', { bubbles: true }));
    });
    const addBtn = Array.from(el.querySelectorAll('button')).find((b) => b.textContent === 'Add') as HTMLButtonElement;
    act(() => addBtn.click());
    await flush();
    expect(el.textContent).toContain('token expired');
  });

  it('selecting a peer fetches and renders that peer\'s state, and back returns to the list', async () => {
    vi.mocked(invoke).mockImplementation((cmd: string, args?: any) => {
      if (cmd === 'observe_list_peers') return Promise.resolve([PEER]);
      if (cmd === 'observe_peer_state') {
        expect(args).toEqual({ peerId: 'peer-1' });
        return Promise.resolve({ generatedAt: 'now', truncated: false, agents: [] });
      }
      return Promise.resolve(undefined);
    });
    const el = render(<PeersPanel />);
    await open(el);

    const peerBtn = el.querySelector('.wd-observe-peer-btn') as HTMLButtonElement;
    act(() => peerBtn.click());
    await flush();

    expect(invoke).toHaveBeenCalledWith('observe_peer_state', { peerId: 'peer-1' });
    expect(el.querySelector('.wd-observe-radar')).not.toBeNull();
    expect(el.textContent).toContain('No active agents');

    const back = el.querySelector('.wd-observe-back') as HTMLButtonElement;
    act(() => back.click());
    await flush();
    expect(el.querySelector('.wd-observe-peer-list')).not.toBeNull();
  });

  it('removing a peer calls observe_remove_peer with its id and drops the selection if it was open', async () => {
    let peers = [PEER];
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'observe_list_peers') return Promise.resolve(peers);
      if (cmd === 'observe_peer_state') return Promise.resolve(null);
      if (cmd === 'observe_remove_peer') {
        peers = [];
        return Promise.resolve(undefined);
      }
      return Promise.resolve(undefined);
    });
    const el = render(<PeersPanel />);
    await open(el);

    const peerBtn = el.querySelector('.wd-observe-peer-btn') as HTMLButtonElement;
    act(() => peerBtn.click());
    await flush();
    expect(el.querySelector('.wd-observe-radar')).not.toBeNull();

    const back = el.querySelector('.wd-observe-back') as HTMLButtonElement;
    act(() => back.click());
    await flush();

    const removeBtn = el.querySelector('.wd-observe-peer-remove') as HTMLButtonElement;
    act(() => removeBtn.click());
    await flush();

    expect(invoke).toHaveBeenCalledWith('observe_remove_peer', { peerId: 'peer-1' });
    expect(el.textContent).toContain('Not watching anyone yet');
  });
});
