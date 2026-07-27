// observeTypes.ts: web-side mirrors of the host/observer management types from
// `src-tauri/src/commands.rs` and `src-tauri/src/observe/{transport,peers}.rs`. These are
// distinct from `observedTypes.ts` (the redacted `ObservedState` wire frame): this file
// covers the LOCAL management surface (grants, peers, sharing status), which never
// crosses the network itself, only IPC between this app and its own Rust backend.

export type HostIdentity = {
  endpointId: string;
  fingerprint: string;
};

export type SharingStatus = {
  sharing: boolean;
  endpointId: string | null;
  fingerprint: string | null;
  liveObservers: number;
};

export type NewGrant = {
  grantId: string;
  /** Shown ONCE. Rust stores only a hash, so there is no way to re-fetch this. */
  token: string;
  expiresAt: string;
};

export type GrantState = 'pending' | 'redeemed' | 'revoked' | 'expired';

export type GrantRow = {
  grantId: string;
  label: string;
  state: GrantState;
  createdAt: string;
  expiresAt: string;
  redeemedFingerprint: string | null;
  lastSeenAt: string | null;
  connected: boolean;
};

export type PendingApproval = {
  connId: string;
  fingerprint: string;
  grantLabel: string;
};

export type PeerRow = {
  peerId: string;
  hostLabel: string;
  fingerprint: string;
  connected: boolean;
  lastFrameAt: string | null;
  error: string | null;
};

/** Grant TTL choices offered in the "mint a grant" form, in seconds. */
export const GRANT_TTL_OPTIONS: readonly { label: string; secs: number }[] = [
  { label: '5 min', secs: 5 * 60 },
  { label: '15 min', secs: 15 * 60 },
  { label: '1 hour', secs: 60 * 60 },
];

const GRANT_STATES: ReadonlySet<string> = new Set(['pending', 'redeemed', 'revoked', 'expired']);

/** Coerce an unknown state string to a known `GrantState`, defaulting to the safest
 * (most restrictive) reading: an unrecognised state never looks active. */
export function toGrantState(v: unknown): GrantState {
  return typeof v === 'string' && GRANT_STATES.has(v) ? (v as GrantState) : 'expired';
}

/** Per-state glyph + label (colour is never the only signal, matching the rest of the
 * radar chrome). */
const GRANT_STATE_META: Record<GrantState, { glyph: string; label: string }> = {
  pending: { glyph: '◐', label: 'Pending' },
  redeemed: { glyph: '●', label: 'Redeemed' },
  revoked: { glyph: '✕', label: 'Revoked' },
  expired: { glyph: '⏱', label: 'Expired' },
};

export function grantStateMeta(state: GrantState): { glyph: string; label: string } {
  return GRANT_STATE_META[state];
}

// ── light normalization for IPC responses. These cross one boundary (this app's own
// Rust backend, a locked and tested contract) rather than a network wire, so this stays
// a thin coercion pass, not the defensive rewrite `observedTypes.ts` does for a frame
// that may have come from a different Warden build entirely. ────────────────────────
function str(v: unknown, fallback = ''): string {
  return typeof v === 'string' ? v : fallback;
}

function strOrNull(v: unknown): string | null {
  return typeof v === 'string' && v.length > 0 ? v : null;
}

export function normalizeHostIdentity(v: any): HostIdentity {
  return { endpointId: str(v?.endpointId), fingerprint: str(v?.fingerprint) };
}

export function normalizeSharingStatus(v: any): SharingStatus {
  return {
    sharing: v?.sharing === true,
    endpointId: strOrNull(v?.endpointId),
    fingerprint: strOrNull(v?.fingerprint),
    liveObservers: typeof v?.liveObservers === 'number' ? Math.max(0, v.liveObservers) : 0,
  };
}

export function normalizeNewGrant(v: any): NewGrant {
  return { grantId: str(v?.grantId), token: str(v?.token), expiresAt: str(v?.expiresAt) };
}

export function normalizeGrantRow(v: any): GrantRow {
  return {
    grantId: str(v?.grantId),
    label: str(v?.label, 'untitled'),
    state: toGrantState(v?.state),
    createdAt: str(v?.createdAt),
    expiresAt: str(v?.expiresAt),
    redeemedFingerprint: strOrNull(v?.redeemedFingerprint),
    lastSeenAt: strOrNull(v?.lastSeenAt),
    connected: v?.connected === true,
  };
}

export function normalizePeerRow(v: any): PeerRow {
  return {
    peerId: str(v?.peerId),
    hostLabel: str(v?.hostLabel, 'unnamed host'),
    fingerprint: str(v?.fingerprint),
    connected: v?.connected === true,
    lastFrameAt: strOrNull(v?.lastFrameAt),
    error: strOrNull(v?.error),
  };
}

/** A rejected `invoke()` in this feature can reject with an `Error`, a plain string (most
 * Tauri command failures), or something stranger; always surface readable text so no
 * caller is tempted to leave a rejection unhandled. */
export function readableError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === 'string') return err;
  return 'something went wrong talking to the backend';
}
