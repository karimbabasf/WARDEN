// ActivationScreen.tsx: the gate's face. Takes a pasted key, hands it to the
// Rust verifier, and either shows the refusal or gets out of the way.
//
// It decides nothing. `license_activate` is the only thing that judges a key,
// which is why there is no format check here and no "looks valid" affordance: a
// second opinion in the webview could only ever refuse a key someone paid for,
// and could never accept one the verifier would not.

import { useCallback, useRef, useState, type FormEvent } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { canSubmit, normalizeStatus, readableActivationError } from './activation';
import './activation.css';

export interface ActivationScreenProps {
  /** Called once, after the backend has accepted and stored the key. */
  onActivated: () => void;
  /** Seam for the dev harness, which has no Tauri backend to talk to. */
  activate?: (key: string) => Promise<unknown>;
}

const PLACEHOLDER = 'WRDN-...';

export function ActivationScreen({ onActivated, activate }: ActivationScreenProps) {
  const [key, setKey] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);

  const submit = useCallback(
    async (e: FormEvent) => {
      e.preventDefault();
      if (!canSubmit(key, busy)) return;
      setBusy(true);
      setError(null);
      try {
        const call = activate ?? ((k: string) => invoke('license_activate', { key: k }));
        const status = normalizeStatus(await call(key.trim()));
        if (!status.activated) {
          // The backend answered without an error and without activating. Not a
          // case that should exist, so it is reported rather than swallowed into
          // a silent no-op that looks like a dead button.
          setError('That key was not accepted. Try again, or reply to your receipt email.');
          return;
        }
        onActivated();
      } catch (err) {
        setError(readableActivationError(err));
        // Put the cursor back where the fix happens.
        inputRef.current?.focus();
        inputRef.current?.select();
      } finally {
        setBusy(false);
      }
    },
    [key, busy, activate, onActivated],
  );

  const ready = canSubmit(key, busy);

  return (
    <div className="wd-activate">
      <main className="wd-activate-card">
        <p className="wd-activate-mark">WARDEN</p>

        <h1 className="wd-activate-title">Enter your license key</h1>
        <p className="wd-activate-lede">
          It came with your receipt and on the page you bought from. One key, one purchase.
        </p>

        <form className="wd-activate-form" onSubmit={submit} noValidate>
          <label className="wd-activate-label" htmlFor="wd-activate-key">
            License key
          </label>
          <textarea
            id="wd-activate-key"
            ref={inputRef}
            className="wd-activate-input"
            value={key}
            onChange={(e) => setKey(e.target.value)}
            onKeyDown={(e) => {
              // A key is one long line, so Enter means submit, not newline.
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                void submit(e);
              }
            }}
            placeholder={PLACEHOLDER}
            rows={3}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            autoComplete="off"
            autoFocus
            disabled={busy}
            aria-invalid={error ? 'true' : undefined}
            aria-describedby={error ? 'wd-activate-error' : undefined}
          />
          <button type="submit" className="wd-activate-submit" disabled={!ready}>
            {busy ? 'Checking...' : 'Activate'}
          </button>
        </form>

        {error ? (
          <p className="wd-activate-error" id="wd-activate-error" role="alert">
            {error}
          </p>
        ) : null}

        <p className="wd-activate-note">
          Checked on this Mac. WARDEN has no activation server and opens no connection to
          verify your key, here or ever.
        </p>
      </main>
    </div>
  );
}

export default ActivationScreen;
