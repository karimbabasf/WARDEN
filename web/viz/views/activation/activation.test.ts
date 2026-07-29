import { describe, expect, it } from 'vitest';
import { canSubmit, normalizeStatus, readableActivationError } from './activation';

describe('readableActivationError', () => {
  it('passes the verifier message through, since it is written for a human', () => {
    expect(readableActivationError('this license key is not valid')).toBe(
      'this license key is not valid',
    );
    expect(readableActivationError('that does not look like a WARDEN license key')).toBe(
      'that does not look like a WARDEN license key',
    );
  });

  it('unwraps an Error', () => {
    expect(readableActivationError(new Error('boom'))).toBe('boom');
  });

  it('never renders an empty or non-string rejection as a blank error', () => {
    for (const junk of [null, undefined, '', '   ', 42, {}, [], new Error('')]) {
      expect(readableActivationError(junk)).toBe('Could not check that key. Try again.');
    }
  });
});

describe('canSubmit', () => {
  it('is false for an empty or whitespace-only field', () => {
    expect(canSubmit('', false)).toBe(false);
    expect(canSubmit('   \n', false)).toBe(false);
  });

  it('is true for anything else, because the verifier is the only judge of a key', () => {
    // Deliberately NOT format-checked here: a client-side shape test would be a
    // second opinion that can refuse a key someone paid for.
    expect(canSubmit('not-even-close', false)).toBe(true);
    expect(canSubmit('WRDN-abc.def', false)).toBe(true);
  });

  it('is false while a check is in flight, so a key cannot be submitted twice', () => {
    expect(canSubmit('WRDN-abc.def', true)).toBe(false);
  });
});

describe('normalizeStatus', () => {
  it('reads a well-formed activated status', () => {
    expect(
      normalizeStatus({ activated: true, gated: true, email: 'a@example.com', seats: 2 }),
    ).toEqual({ activated: true, gated: true, email: 'a@example.com', seats: 2 });
  });

  it('FAILS CLOSED on anything it does not recognize', () => {
    for (const junk of [null, undefined, {}, [], 'yes', 0, { activated: 'true' }, { activated: 1 }]) {
      expect(normalizeStatus(junk).activated).toBe(false);
    }
  });

  it('drops fields of the wrong type rather than passing them to the view', () => {
    const s = normalizeStatus({ activated: true, gated: true, email: 12, seats: 'two' });
    expect(s.email).toBeNull();
    expect(s.seats).toBeNull();
  });
});
