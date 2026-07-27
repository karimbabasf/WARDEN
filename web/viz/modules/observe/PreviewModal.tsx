// PreviewModal.tsx: "preview what others see," mounted from the SHARE tab. Fetches
// `preview_observed_state` (the SAME projection the wire uses, per `commands.rs`) and
// renders it through `ObservedRadarView`, the identical component the observer's own
// dock uses. There is no second formatter here on purpose: a preview that could drift
// from the real thing would manufacture false confidence at the exact moment someone
// decides whether to share.
//
// Unlike `ApprovalModal`, this is a plain dismissable preview (no security decision is
// pending), so Escape and an outside click both close it.

import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { normalizeObservedState, type ObservedState } from '@/viz/shared/types/observedTypes';
import { ObservedRadarView } from './ObservedRadarView';
import { readableError } from './observeTypes';

export function PreviewModal({ onClose }: { onClose: () => void }) {
  const [state, setState] = useState<ObservedState | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    invoke('preview_observed_state')
      .then((raw) => {
        if (!cancelled) setState(normalizeObservedState(raw));
      })
      .catch((err) => {
        if (!cancelled) setError(readableError(err));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === 'Escape') onClose();
    }
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);

  return (
    <div className="wd-observe-preview-scrim" onClick={onClose} role="presentation">
      <div className="wd-observe-preview" role="dialog" aria-modal="true" onClick={(e) => e.stopPropagation()}>
        <button type="button" className="wd-observe-preview-close" onClick={onClose} aria-label="Close preview">
          ✕
        </button>
        {error ? (
          <p className="wd-observe-error" role="alert">
            {error}
          </p>
        ) : (
          <ObservedRadarView
            state={state}
            title="Preview: what an observer would see"
            subtitle="Rendered through the exact projection the wire uses."
          />
        )}
      </div>
    </div>
  );
}

export default PreviewModal;
