#!/usr/bin/env node
// scripts/release.mjs
//
// Ships a WARDEN release: build the app, produce the DMG, hash it, publish it
// as a GitHub release asset on karimbabasf/WARDEN. Reproducible: run this one
// command for every version.
//
// Safety: everything up to and including "release plan" is read-only and
// local. Nothing talks to GitHub unless --confirm is passed, and every gh
// invocation in this file lives inside the `if (CONFIRM)` block below, so
// that guarantee is a property of the code, not just of good intentions.
//
// Signing: set APPLE_SIGNING_IDENTITY to sign for real instead of ad-hoc.
// Add APPLE_ID + APPLE_PASSWORD + APPLE_TEAM_ID on top of that to also
// notarize. Tauri's own macOS bundler does the signing, notarization and
// stapling itself inside `tauri build` when it finds these env vars -- see
// https://v2.tauri.app/distribute/sign/macos/ ("Notarization" section: set
// APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID, rerun `tauri build`). This script
// does not shell out to notarytool a second time: the installed notarytool
// (`xcrun notarytool submit --help`, v1.1.2 on this machine) takes exactly
// those three credentials plus --wait, which confirms Tauri is driving the
// real thing and not something stale -- re-submitting the same DMG again
// after Tauri already notarized it would just waste a second trip to Apple's
// notary service. Instead, this script verifies the outcome: codesign after
// every build, and `stapler validate` whenever notarization was expected.
// With no APPLE_SIGNING_IDENTITY, the build falls back to the current
// ad-hoc/linker-signed build and this script says so plainly.
//
// Usage:
//   node scripts/release.mjs                build, hash, print the plan (dry run)
//   node scripts/release.mjs --confirm      also create/update the GitHub release
//   node scripts/release.mjs --tag v0.1.0   override the tag (default v<tauri.conf version>)
//   node scripts/release.mjs --notes "..."  override the generated release notes
//   node scripts/release.mjs --skip-build   reuse the existing dmg (iterating on this script)

import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream, existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const REPO = 'karimbabasf/WARDEN';

const args = process.argv.slice(2);
const CONFIRM = args.includes('--confirm');
const SKIP_BUILD = args.includes('--skip-build');
const tagArg = flagValue('--tag');
const notesArg = flagValue('--notes');

function flagValue(flag) {
  const i = args.indexOf(flag);
  return i === -1 ? undefined : args[i + 1];
}

function run(cmd, cmdArgs) {
  console.log(`$ ${cmd} ${cmdArgs.join(' ')}`);
  const result = spawnSync(cmd, cmdArgs, { stdio: 'inherit', cwd: ROOT, env: process.env });
  if (result.status !== 0) {
    throw new Error(`${cmd} ${cmdArgs.join(' ')} exited ${result.status}`);
  }
}

function runCapture(cmd, cmdArgs) {
  return spawnSync(cmd, cmdArgs, { cwd: ROOT, encoding: 'utf8', env: process.env });
}

function hashFile(file) {
  return new Promise((resolve, reject) => {
    const hash = createHash('sha256');
    const stream = createReadStream(file);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.on('end', () => resolve(hash.digest('hex')));
    stream.on('error', reject);
  });
}

function mb(bytes) {
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

async function main() {
  const tauriConf = JSON.parse(readFileSync(path.join(ROOT, 'src-tauri/tauri.conf.json'), 'utf8'));
  const PRODUCT_NAME = tauriConf.productName;
  const VERSION = tauriConf.version;
  const TAG = tagArg ?? `v${VERSION}`;

  const signingIdentity = process.env.APPLE_SIGNING_IDENTITY;
  const appleId = process.env.APPLE_ID;
  const applePassword = process.env.APPLE_PASSWORD;
  const appleTeamId = process.env.APPLE_TEAM_ID;
  const SIGNED = Boolean(signingIdentity);
  const NOTARIZE = Boolean(signingIdentity && appleId && applePassword && appleTeamId);

  console.log('=== WARDEN release ===');
  console.log(`Repo:         ${REPO}`);
  console.log(`Product:      ${PRODUCT_NAME} ${VERSION}`);
  console.log(`Tag:          ${TAG}`);
  console.log(`Signing:      ${SIGNED ? `real (${signingIdentity})` : 'ad-hoc (APPLE_SIGNING_IDENTITY not set)'}`);
  console.log(`Notarization: ${NOTARIZE ? 'on (Tauri notarizes + staples during build)' : 'off'}`);
  if (SIGNED && !NOTARIZE) {
    console.log('NOTE: APPLE_SIGNING_IDENTITY is set but APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID are not all set: signed, not notarized.');
  }
  if (!SIGNED) {
    console.log('NOTICE: unsigned build ahead. Gatekeeper will call this DMG damaged on first open; docs/INSTALL.md has the right-click-Open fix.');
  }

  const bundleDmgDir = path.join(ROOT, 'src-tauri/target/release/bundle/dmg');
  if (!SKIP_BUILD) {
    console.log('\n=== building ===');
    run('pnpm', ['tauri', 'build', '--bundles', 'app,dmg']);
  } else {
    console.log('\n=== --skip-build: reusing existing bundle output ===');
  }

  if (!existsSync(bundleDmgDir)) {
    throw new Error(`no dmg output directory at ${bundleDmgDir}; the build did not produce one`);
  }
  const dmgFiles = readdirSync(bundleDmgDir).filter((f) => f.endsWith('.dmg'));
  if (dmgFiles.length !== 1) {
    throw new Error(`expected exactly one .dmg in ${bundleDmgDir}, found ${dmgFiles.length}: ${dmgFiles.join(', ') || '(none)'}`);
  }
  const dmgPath = path.join(bundleDmgDir, dmgFiles[0]);
  const dmgSize = statSync(dmgPath).size;
  const sha256 = await hashFile(dmgPath);

  console.log('\n=== artifact ===');
  console.log(`File:     ${dmgPath}`);
  console.log(`Size:     ${mb(dmgSize)}`);
  console.log(`SHA-256:  ${sha256}`);

  console.log('\n=== signature verification ===');
  const appPath = path.join(ROOT, 'src-tauri/target/release/bundle/macos', `${PRODUCT_NAME}.app`);
  if (existsSync(appPath)) {
    const codesignResult = runCapture('codesign', ['-dv', '--verbose=2', appPath]);
    const codesignOutput = `${codesignResult.stdout}${codesignResult.stderr}`.trim();
    console.log(codesignOutput);
    const isAdhoc = /Signature=adhoc/.test(codesignOutput);
    if (SIGNED && isAdhoc) {
      throw new Error('APPLE_SIGNING_IDENTITY was set but the built app is still ad-hoc signed. Check the identity with `security find-identity -v -p codesigning`.');
    }
    if (!SIGNED && !isAdhoc) {
      console.log('NOTE: build is signed even though APPLE_SIGNING_IDENTITY was not set in this run (a cached identity or config is in play).');
    }
  } else {
    console.log(`(no .app bundle at ${appPath}; skipping codesign check)`);
  }

  if (NOTARIZE) {
    const staple = runCapture('xcrun', ['stapler', 'validate', dmgPath]);
    console.log(`${staple.stdout}${staple.stderr}`.trim());
    if (staple.status !== 0) {
      throw new Error('APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID were set but the DMG has no valid stapled ticket. Check the tauri build log above for a notarization failure.');
    }
    console.log('Notarization ticket verified and stapled.');
  }

  const title = `${PRODUCT_NAME} ${TAG}`;
  const notes = notesArg ?? defaultNotes({ PRODUCT_NAME, VERSION, SIGNED, NOTARIZE, sha256 });

  console.log('\n=== release plan ===');
  console.log(`Title:  ${title}`);
  console.log(`Asset:  ${path.basename(dmgPath)} (${mb(dmgSize)})`);
  console.log('Notes:');
  for (const line of notes.split('\n')) console.log(`  ${line}`);

  if (!CONFIRM) {
    console.log('\nDry run only: no GitHub call was made (every `gh` invocation in this script is gated on --confirm). Pass --confirm to publish.');
    return;
  }

  // --- everything below this line is the only part of the script that talks to GitHub ---
  console.log('\n=== publishing (--confirm passed) ===');
  const existing = runCapture('gh', ['release', 'view', TAG, '--repo', REPO, '--json', 'tagName']);
  const releaseExists = existing.status === 0;
  console.log(`Existing release ${TAG} on ${REPO}: ${releaseExists ? 'yes, uploading asset with --clobber' : 'no, creating it'}`);

  if (releaseExists) {
    run('gh', ['release', 'upload', TAG, dmgPath, '--repo', REPO, '--clobber']);
  } else {
    run('gh', ['release', 'create', TAG, dmgPath, '--repo', REPO, '--title', title, '--notes', notes]);
  }

  const verify = runCapture('gh', ['release', 'view', TAG, '--repo', REPO, '--json', 'assets,url']);
  console.log(verify.stdout.trim());
  console.log(`\nPublished ${TAG} to ${REPO}.`);
}

function defaultNotes({ PRODUCT_NAME, VERSION, SIGNED, NOTARIZE, sha256 }) {
  const signedLine = NOTARIZE
    ? 'Signed with a Developer ID and notarized by Apple.'
    : SIGNED
      ? 'Signed with a Developer ID (not notarized).'
      : 'Ad-hoc signed only (unsigned). Gatekeeper will call it damaged on first open -- see docs/INSTALL.md for the one-time fix.';
  return `${PRODUCT_NAME} ${VERSION}, macOS Apple Silicon (arm64) only.\n\n${signedLine}\n\nSHA-256: ${sha256}`;
}

main().catch((err) => {
  console.error(`\nERROR: ${err.message}`);
  process.exit(1);
});
