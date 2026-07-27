// @vitest-environment jsdom
//
// ShareMenu covers the host's SHARE/ACCESS popover. The two load-bearing behaviours: a
// freshly minted token is shown exactly once (with a copy affordance and no way to
// re-fetch it), and revoking a grant from the ACCESS tab calls `observe_revoke_grant`
// with that grant's id, never a stale one.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { ShareMenu } from './ShareMenu';

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

const SHARING_STATUS = { sharing: true, endpointId: 'ep-1', fingerprint: 'AB12:CD34:EF56:0000', liveObservers: 0 };

beforeEach(() => {
  vi.mocked(invoke).mockReset();
  vi.mocked(listen).mockReset();
  vi.mocked(listen).mockReturnValue(Promise.resolve(() => {}));
  vi.mocked(invoke).mockImplementation((cmd: string) => {
    switch (cmd) {
      case 'observe_sharing_status':
        return Promise.resolve(SHARING_STATUS);
      case 'observe_list_grants':
        return Promise.resolve([]);
      default:
        return Promise.resolve(undefined);
    }
  });
  Object.assign(navigator, { clipboard: { writeText: vi.fn(() => Promise.resolve()) } });
});

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
  vi.restoreAllMocks();
});

async function openShareTab(el: HTMLElement) {
  const trigger = el.querySelector<HTMLButtonElement>('.wd-observe-trigger');
  expect(trigger).not.toBeNull();
  act(() => trigger!.click());
  await flush();
}

describe('ShareMenu', () => {
  it('shows the sharing toggle and, once sharing, the host fingerprint', async () => {
    const el = render(<ShareMenu />);
    await openShareTab(el);
    expect(el.querySelector('.wd-observe-switch.is-on')).not.toBeNull();
    expect(el.textContent).toContain('AB12:CD34:EF56:0000');
  });

  it('shows a newly minted token exactly once, with a copy control', async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      switch (cmd) {
        case 'observe_sharing_status':
          return Promise.resolve(SHARING_STATUS);
        case 'observe_list_grants':
          return Promise.resolve([]);
        case 'observe_create_grant':
          return Promise.resolve({ grantId: 'g1', token: 'once-only-secret-token', expiresAt: '2026-07-26T12:15:00Z' });
        default:
          return Promise.resolve(undefined);
      }
    });

    const el = render(<ShareMenu />);
    await openShareTab(el);

    const labelInput = el.querySelector<HTMLInputElement>('.wd-observe-form input[type="text"]');
    expect(labelInput).not.toBeNull();
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
    act(() => {
      setter?.call(labelInput, 'Askhat');
      labelInput!.dispatchEvent(new Event('input', { bubbles: true }));
    });

    const form = el.querySelector('form.wd-observe-form') as HTMLFormElement;
    act(() => form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })));
    await flush();

    expect(invoke).toHaveBeenCalledWith('observe_create_grant', { label: 'Askhat', ttlSecs: expect.any(Number) });

    const tokenEl = el.querySelector('[data-new-grant] code');
    expect(tokenEl?.textContent).toBe('once-only-secret-token');
    expect(el.querySelector('[data-new-grant]')?.textContent).toContain('shown once');

    const copyBtn = Array.from(el.querySelectorAll('[data-new-grant] button')).find((b) => b.textContent === 'Copy') as HTMLButtonElement;
    expect(copyBtn).toBeDefined();
    act(() => copyBtn.click());
    await flush();
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith('once-only-secret-token');
    expect(el.querySelector('[data-new-grant]')?.textContent).toContain('Copied');

    // No command exists to re-fetch the raw token: it only ever lived in this response.
    expect(vi.mocked(invoke).mock.calls.filter(([cmd]) => cmd === 'observe_create_grant')).toHaveLength(1);
  });

  it('revoking a grant in the ACCESS tab calls observe_revoke_grant with that grant\'s id', async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      switch (cmd) {
        case 'observe_sharing_status':
          return Promise.resolve(SHARING_STATUS);
        case 'observe_list_grants':
          return Promise.resolve([
            { grantId: 'g1', label: 'Askhat', state: 'pending', createdAt: 'now', expiresAt: 'later', redeemedFingerprint: null, lastSeenAt: null, connected: false },
          ]);
        case 'observe_revoke_grant':
          return Promise.resolve(undefined);
        default:
          return Promise.resolve(undefined);
      }
    });

    const el = render(<ShareMenu />);
    await openShareTab(el);

    const accessTabBtn = Array.from(el.querySelectorAll('.wd-observe-tab')).find((b) => b.textContent === 'Access') as HTMLButtonElement;
    act(() => accessTabBtn.click());
    await flush();

    const revokeBtn = el.querySelector('[data-grant-row="g1"] button') as HTMLButtonElement;
    expect(revokeBtn.textContent).toBe('Revoke');
    act(() => revokeBtn.click());
    await flush();

    expect(invoke).toHaveBeenCalledWith('observe_revoke_grant', { grantId: 'g1' });
  });

  it('catches a status failure and renders a readable message instead of an unhandled rejection', async () => {
    vi.mocked(invoke).mockImplementation((cmd: string) => {
      if (cmd === 'observe_sharing_status') return Promise.reject('backend unreachable');
      return Promise.resolve(cmd === 'observe_list_grants' ? [] : undefined);
    });
    const el = render(<ShareMenu />);
    // If the rejection above were left unhandled, vitest would report it against this
    // test regardless of these assertions; the rendered message is the caught path.
    await openShareTab(el);
    expect(el.textContent).toContain('backend unreachable');
  });
});
