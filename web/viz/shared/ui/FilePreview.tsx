// FilePreview.tsx: a bounded peek at the file an agent is touching right now.
//
// Lives in `shared/` rather than a module because both the radar detail panel
// and the activity feed reach for it, and FSD forbids sibling-module imports.
//
// Honest by construction: the backend refuses any path that is not currently on
// the radar, so a preview that fails says so plainly instead of rendering an
// empty box that reads like an empty file. Those two states look different on
// purpose.

import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';

export type FilePreviewData = {
  path: string;
  text: string;
  truncated: boolean;
  totalBytes: number;
  shownLines: number;
  isText: boolean;
};

function normalize(raw: any): FilePreviewData {
  return {
    path: typeof raw?.path === 'string' ? raw.path : '',
    text: typeof raw?.text === 'string' ? raw.text : '',
    truncated: raw?.truncated === true,
    totalBytes: typeof raw?.totalBytes === 'number' ? raw.totalBytes : 0,
    shownLines: typeof raw?.shownLines === 'number' ? raw.shownLines : 0,
    isText: raw?.isText !== false,
  };
}

/** Byte count for the footer: 812 B, 41.2 KB, 3.1 MB. */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0 B';
  if (n < 1024) return `${Math.round(n)} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

type LoadState =
  | { phase: 'loading' }
  | { phase: 'ready'; data: FilePreviewData }
  | { phase: 'error'; message: string };

/**
 * Fetches on mount and whenever `path` changes. A stale response from a previous
 * path is discarded via a token, so clicking quickly through three activity rows
 * cannot leave the third row showing the first row's file.
 */
export function FilePreview({ path, onClose }: { path: string; onClose?: () => void }) {
  const [state, setState] = useState<LoadState>({ phase: 'loading' });
  const tokenRef = useRef(0);

  const load = useCallback(() => {
    const token = ++tokenRef.current;
    setState({ phase: 'loading' });
    invoke('preview_file', { path })
      .then((raw) => {
        if (tokenRef.current !== token) return;
        setState({ phase: 'ready', data: normalize(raw) });
      })
      .catch((err: unknown) => {
        if (tokenRef.current !== token) return;
        setState({ phase: 'error', message: typeof err === 'string' ? err : 'could not read that file' });
      });
  }, [path]);

  useEffect(() => {
    load();
  }, [load]);

  return (
    <div className="wd-fileview" data-file-preview={path}>
      <div className="wd-fileview-head">
        <span className="wd-fileview-path" title={path}>
          {path}
        </span>
        {onClose ? (
          <button type="button" className="wd-fileview-close" onClick={onClose} aria-label="Close preview">
            ×
          </button>
        ) : null}
      </div>

      {state.phase === 'loading' ? (
        /* A skeleton shaped like code, not a spinner: it occupies the height the
           real content will, so opening a preview never shifts the panel. */
        <div className="wd-fileview-skeleton" aria-label="Loading preview">
          <span /><span /><span /><span /><span />
        </div>
      ) : state.phase === 'error' ? (
        <p className="wd-fileview-error" role="alert">
          {state.message}
          <button type="button" className="wd-fileview-retry" onClick={load}>
            Retry
          </button>
        </p>
      ) : !state.data.isText ? (
        <p className="wd-fileview-error">
          Binary file, {formatBytes(state.data.totalBytes)}. Nothing to show.
        </p>
      ) : (
        <>
          <pre className="wd-fileview-body">
            <code>{state.data.text}</code>
          </pre>
          <div className="wd-fileview-foot">
            <span>
              {state.data.shownLines} line{state.data.shownLines === 1 ? '' : 's'}
            </span>
            <span>{formatBytes(state.data.totalBytes)}</span>
            {state.data.truncated ? <span className="wd-fileview-trunc">truncated</span> : null}
          </div>
        </>
      )}
    </div>
  );
}

export default FilePreview;
