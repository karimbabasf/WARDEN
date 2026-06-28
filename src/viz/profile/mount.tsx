// mount.tsx — mounts the full-page DOSSIER ProfileScreen into its OWN React root,
// completely separate from the war-room island so the warm R3F mount is never
// touched. The Profile overlay is opaque and z-indexed above the war room: when
// open it simply covers the canvas; when closed the root renders nothing and the
// war room is visible/interactive again underneath.
//
// `openProfile()` / `closeProfile()` are the imperative seam main.ts drives (it
// is the single Tauri router and owns no React otherwise). Mount is lazy: the
// root is created on first open so the Profile's React tree costs nothing until
// the operator actually summons it.

import { createRoot, type Root } from 'react-dom/client';
import { ProfileScreen } from './ProfileScreen';

let root: Root | null = null;
let mountedOpen = false;

function ensureRoot(): Root {
  if (root) return root;
  let el = document.getElementById('profile-root');
  if (!el) {
    // Self-heal if index.html lacks the node (e.g. dev HMR edge) so the surface
    // still works rather than throwing.
    el = document.createElement('div');
    el.id = 'profile-root';
    document.body.appendChild(el);
  }
  root = createRoot(el);
  return root;
}

/** Render the full-page Profile over everything. Idempotent while open. */
export function openProfile(): void {
  if (mountedOpen) return;
  mountedOpen = true;
  ensureRoot().render(<ProfileScreen onClose={closeProfile} />);
}

/** Hide the Profile (renders nothing; war room shows through). Idempotent. */
export function closeProfile(): void {
  if (!mountedOpen) return;
  mountedOpen = false;
  root?.render(null);
}

/** Flip open↔closed. Returns the new state (true = now open). */
export function toggleProfile(): boolean {
  if (mountedOpen) {
    closeProfile();
    return false;
  }
  openProfile();
  return true;
}

export function isProfileOpen(): boolean {
  return mountedOpen;
}
