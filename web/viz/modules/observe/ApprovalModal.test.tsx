// @vitest-environment jsdom
//
// ApprovalModal is the one gate between a redeemed token and an actual live connection.
// The load-bearing property is that it cannot be dismissed except by an explicit Approve
// or Deny: no outside click, no Escape, no timeout. Anything else would turn an
// intercepted token into a silent grant (see the file header in ApprovalModal.tsx).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { ApprovalModal } from './ApprovalModal';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => Promise.resolve(undefined)) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

let container: HTMLDivElement | null = null;
let root: Root | null = null;
let approvalHandler: ((e: { payload: unknown }) => void) | null = null;

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

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    if (cmd === 'observe_pending_approvals') return Promise.resolve([]);
    return Promise.resolve(undefined);
  });
  approvalHandler = null;
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockImplementation((event: string, handler: any) => {
    if (event === 'observe:approval') approvalHandler = handler;
    return Promise.resolve(() => {});
  });
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
});

const APPROVAL = { connId: 'conn-1', fingerprint: 'AB12:CD34:EF56:0000', grantLabel: 'Askhat' };

describe('ApprovalModal', () => {
  it('renders nothing while no connection is waiting', async () => {
    const el = render(<ApprovalModal />);
    await flush();
    expect(el.querySelector('.wd-observe-approval')).toBeNull();
  });

  it('shows the fingerprint and grant label when observe:approval fires', async () => {
    const el = render(<ApprovalModal />);
    await flush();
    act(() => approvalHandler!({ payload: APPROVAL }));
    await flush();
    expect(el.querySelector('.wd-observe-approval')).not.toBeNull();
    expect(el.textContent).toContain('AB12:CD34:EF56:0000');
    expect(el.textContent).toContain('Askhat');
  });

  it('does not close on an outside click', async () => {
    const el = render(<ApprovalModal />);
    await flush();
    act(() => approvalHandler!({ payload: APPROVAL }));
    await flush();

    act(() => document.body.dispatchEvent(new MouseEvent('mousedown', { bubbles: true })));
    act(() => document.body.click());
    await flush();
    expect(el.querySelector('.wd-observe-approval')).not.toBeNull();
  });

  it('does not close on Escape', async () => {
    const el = render(<ApprovalModal />);
    await flush();
    act(() => approvalHandler!({ payload: APPROVAL }));
    await flush();

    act(() => window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })));
    await flush();
    expect(el.querySelector('.wd-observe-approval')).not.toBeNull();
  });

  it('Approve calls observe_resolve_approval with approve: true for that connection', async () => {
    const el = render(<ApprovalModal />);
    await flush();
    act(() => approvalHandler!({ payload: APPROVAL }));
    await flush();

    const approveBtn = el.querySelector('.wd-observe-btn-approve') as HTMLButtonElement;
    act(() => approveBtn.click());
    await flush();

    expect(invoke).toHaveBeenCalledWith('observe_resolve_approval', { connId: 'conn-1', approve: true });
    expect(el.querySelector('.wd-observe-approval')).toBeNull();
  });

  it('Deny calls observe_resolve_approval with approve: false', async () => {
    const el = render(<ApprovalModal />);
    await flush();
    act(() => approvalHandler!({ payload: APPROVAL }));
    await flush();

    const denyBtn = el.querySelector('.wd-observe-btn-deny') as HTMLButtonElement;
    act(() => denyBtn.click());
    await flush();

    expect(invoke).toHaveBeenCalledWith('observe_resolve_approval', { connId: 'conn-1', approve: false });
    expect(el.querySelector('.wd-observe-approval')).toBeNull();
  });

  it('seeds the queue from observe_pending_approvals on mount', async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'observe_pending_approvals') return Promise.resolve([APPROVAL]);
      return Promise.resolve(undefined);
    });
    const el = render(<ApprovalModal />);
    await flush();
    expect(el.querySelector('.wd-observe-approval')).not.toBeNull();
    expect(el.textContent).toContain('Askhat');
  });

  it('catches a resolve rejection and keeps the approval visible to retry, instead of an unhandled rejection', async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'observe_pending_approvals') return Promise.resolve([]);
      if (cmd === 'observe_resolve_approval') return Promise.reject('connection already closed');
      return Promise.resolve(undefined);
    });

    const el = render(<ApprovalModal />);
    await flush();
    act(() => approvalHandler!({ payload: APPROVAL }));
    await flush();
    const approveBtn = el.querySelector('.wd-observe-btn-approve') as HTMLButtonElement;
    act(() => approveBtn.click());
    await flush();

    // If the rejection above were left unhandled, vitest would report it against this
    // test regardless of these assertions.
    expect(el.textContent).toContain('connection already closed');
    expect(el.querySelector('.wd-observe-approval')).not.toBeNull();
  });
});
