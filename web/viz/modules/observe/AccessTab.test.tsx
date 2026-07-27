// @vitest-environment jsdom
//
// AccessTab is presentational: ShareMenu owns the fetch/refresh/revoke plumbing. The
// load-bearing assertion is that clicking Revoke on a row calls back with that row's
// grantId, never a stale or wrong one, and that a row which can no longer be revoked
// (already revoked or expired) shows no Revoke control at all.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { AccessTab } from './AccessTab';
import type { GrantRow } from './observeTypes';

function grantFixture(over: Partial<GrantRow> = {}): GrantRow {
  return {
    grantId: 'grant-1',
    label: 'Askhat',
    state: 'pending',
    createdAt: '2026-07-26T12:00:00Z',
    expiresAt: '2026-07-26T12:15:00Z',
    redeemedFingerprint: null,
    lastSeenAt: null,
    connected: false,
    ...over,
  };
}

let container: HTMLDivElement | null = null;
let root: Root | null = null;

function render(node: ReactNode): HTMLElement {
  container = document.createElement('div');
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(node));
  return container;
}

afterEach(() => {
  act(() => root?.unmount());
  container?.remove();
  root = null;
  container = null;
});

describe('AccessTab', () => {
  it('shows an honest empty state with no grants', () => {
    const el = render(<AccessTab grants={[]} error={null} revokingId={null} onRevoke={() => {}} />);
    expect(el.textContent).toContain('No grants minted yet');
  });

  it('calls onRevoke with the clicked row\'s grantId, not any other row\'s', () => {
    const onRevoke = vi.fn();
    const grants = [grantFixture({ grantId: 'g1', label: 'Askhat' }), grantFixture({ grantId: 'g2', label: 'Other friend', state: 'redeemed' })];
    const el = render(<AccessTab grants={grants} error={null} revokingId={null} onRevoke={onRevoke} />);
    const buttons = Array.from(el.querySelectorAll('[data-grant-row="g2"] button')) as HTMLButtonElement[];
    expect(buttons).toHaveLength(1);
    act(() => buttons[0].click());
    expect(onRevoke).toHaveBeenCalledTimes(1);
    expect(onRevoke).toHaveBeenCalledWith('g2');
  });

  it('renders no Revoke control for a row that is already revoked or expired', () => {
    const grants = [grantFixture({ grantId: 'g1', state: 'revoked' }), grantFixture({ grantId: 'g2', state: 'expired' })];
    const el = render(<AccessTab grants={grants} error={null} revokingId={null} onRevoke={() => {}} />);
    expect(el.querySelector('[data-grant-row="g1"] button')).toBeNull();
    expect(el.querySelector('[data-grant-row="g2"] button')).toBeNull();
  });

  it('shows the connected dot only for a connected row', () => {
    const grants = [grantFixture({ grantId: 'g1', connected: true }), grantFixture({ grantId: 'g2', connected: false })];
    const el = render(<AccessTab grants={grants} error={null} revokingId={null} onRevoke={() => {}} />);
    expect(el.querySelector('[data-grant-row="g1"] .wd-observe-dot.is-connected')).not.toBeNull();
    expect(el.querySelector('[data-grant-row="g2"] .wd-observe-dot.is-connected')).toBeNull();
  });

  it('surfaces a list-level error without crashing the table', () => {
    const el = render(<AccessTab grants={[]} error="could not reach the host" revokingId={null} onRevoke={() => {}} />);
    expect(el.textContent).toContain('could not reach the host');
  });
});
