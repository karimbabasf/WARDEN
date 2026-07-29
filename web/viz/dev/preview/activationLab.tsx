// activationLab.tsx: the activation screen, served by vite at /activation-lab.html.
//
// The screen is the first thing a buyer ever sees and it is otherwise only
// reachable from a RELEASE build with no stored key, which is not a state a
// browser pass can get to. This harness mounts the real component against a
// stubbed verifier so the states that matter can be looked at directly:
//
//   /activation-lab.html              the empty first-launch state
//   /activation-lab.html?state=error  the refused-key state
//
// The stub only ever REFUSES, and it returns the verifier's own wording. There
// is deliberately no way to fake a success here: a harness that could open the
// radar without a signature would be the exact bypass the gate exists to
// prevent, even in dev.

import { createRoot } from 'react-dom/client';
import { ActivationScreen } from '@/viz/views/activation/ActivationScreen';
import '@/style.css';

const params = new URLSearchParams(window.location.search);

/** Mirrors what `license_activate` rejects with, string for string. */
const refuse = (key: string): Promise<never> =>
  Promise.reject(
    key.trim().startsWith('WRDN-')
      ? 'this license key is not valid'
      : 'that does not look like a WARDEN license key',
  );

const el = document.getElementById('war-room-root');
if (!el) throw new Error('activationLab: #war-room-root not found');

const root = createRoot(el);
root.render(
  <ActivationScreen
    activate={refuse}
    onActivated={() => console.warn('activationLab: the stub never activates')}
  />,
);

// `?state=error` drives the real component into its error state through the real
// submit path, rather than rendering a hand-made copy of it that could drift.
//
// It has to WAIT for the mount: `root.render` commits asynchronously, so a
// microtask scheduled here runs while the field still does not exist. The first
// version of this silently no-opped and the screenshot showed a pristine empty
// form, which is exactly the kind of quiet harness failure that makes a browser
// pass worthless. Hence the explicit frame loop and the loud give-up.
if (params.get('state') === 'error') {
  const REJECTED_KEY = 'WRDN-eyJ2IjoxfQ.notarealsignature';
  let frames = 0;

  const drive = () => {
    const field = document.getElementById('wd-activate-key') as HTMLTextAreaElement | null;
    const form = field?.closest('form');
    if (!field || !form) {
      if (frames++ > 120) throw new Error('activationLab: the form never mounted');
      requestAnimationFrame(drive);
      return;
    }
    // React tracks its own value on the node, so setting `.value` directly is
    // ignored on the next render. Go through the prototype setter the way
    // testing-library does, then fire the event React actually listens for.
    Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, 'value')?.set?.call(
      field,
      REJECTED_KEY,
    );
    field.dispatchEvent(new Event('input', { bubbles: true }));
    requestAnimationFrame(() => form.requestSubmit());
  };

  requestAnimationFrame(drive);
}
