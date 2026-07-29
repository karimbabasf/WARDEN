// activation.ts: the two pure decisions the activation screen makes, kept out of
// the component so they can be asserted without a DOM.
//
// Note what is NOT here: any judgement about whether a key is valid. The Rust
// verifier is the only authority on that, and a client-side format check would
// only create a second opinion that can refuse a key someone actually paid for.
// The most this file will say is "the field is empty".

/** Tauri rejects a command with whatever the Rust side passed to `reject`, which
 *  arrives as a bare string. Anything else is an unexpected transport failure. */
export function readableActivationError(err: unknown): string {
  if (typeof err === 'string' && err.trim()) return err.trim();
  if (err instanceof Error && err.message.trim()) return err.message.trim();
  return 'Could not check that key. Try again.';
}

/** Enables the submit button. Deliberately only a non-empty test: see the note
 *  above about not second-guessing the verifier. */
export function canSubmit(raw: string, busy: boolean): boolean {
  return !busy && raw.trim().length > 0;
}

/** The shape `license_status` and `license_activate` return. */
export interface LicenseStatus {
  activated: boolean;
  gated: boolean;
  email: string | null;
  seats: number | null;
}

/** Read a status off the wire without trusting its shape.
 *
 *  Fails CLOSED: anything unrecognizable counts as not activated. A gate that
 *  defaults to open on a malformed reply is not a gate. */
export function normalizeStatus(v: unknown): LicenseStatus {
  const o = (v ?? {}) as Record<string, unknown>;
  return {
    activated: o.activated === true,
    gated: o.gated === true,
    email: typeof o.email === 'string' ? o.email : null,
    seats: typeof o.seats === 'number' ? o.seats : null,
  };
}
