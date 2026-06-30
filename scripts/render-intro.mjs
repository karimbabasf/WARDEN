#!/usr/bin/env node
//
// render-intro.mjs — POST-M2 STUB (intentionally inert in M2).
//
// ── Why this is a stub ───────────────────────────────────────────────────────
// Spec §6.5 lists three Remotion roles. Role 2 ("branded intro") is described as
// a build-time PRE-RENDERED asset. True pre-render — turning the <Intro/>
// composition into an .mp4/.webm file on disk at build time — requires
// `@remotion/renderer` (the headless Chromium render backend). Risk **R-Rem**
// explicitly DEFERS `@remotion/renderer` to post-M2: it pulls a ~150MB headless
// browser, lengthens CI, and is not needed to ship the experience.
//
// ── What M2 ships instead (equivalent from the user's seat) ──────────────────
// In M2 the intro is played LIVE through `@remotion/player`'s <Player> on first
// summon (see web/viz/PlayerHost.tsx + web/viz/WarRoom.tsx). Playing the
// composition live needs NO render backend and is frame-identical to a
// pre-render — the user sees the same branded boot, just rendered on the fly.
// The compositions are deterministic, so when this script is implemented it will
// produce a file that matches the live playback to the frame.
//
// ── How to implement post-M2 (do NOT do this in M2) ──────────────────────────
//   1. pnpm add -D @remotion/renderer @remotion/bundler @remotion/cli
//   2. Wrap web/viz/compositions/Intro.tsx in a <Composition> inside a Remotion
//      entry (e.g. web/viz/remotion-root.tsx) registered via registerRoot().
//   3. bundle() that entry, selectComposition('Intro'), then renderMedia({
//        codec: 'vp9', outputLocation: 'web/assets/intro.webm', ...
//      }).
//   4. Commit the rendered web/assets/intro.webm and have WarRoom prefer the
//      pre-rendered asset (instant first frame) with the <Player> as fallback.
//
// Running it now is a NO-OP that documents the above and exits 0 so it can sit
// in package scripts / CI without failing.

console.log('[render-intro] POST-M2 STUB — no pre-render in M2.');
console.log('[render-intro] The branded intro plays LIVE via @remotion/player <Player>');
console.log('[render-intro] on first summon (web/viz/PlayerHost.tsx). No @remotion/renderer.');
console.log('[render-intro] See this file\'s header for the post-M2 implementation path.');
process.exit(0);
