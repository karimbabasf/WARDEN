# WARDEN in boring.notch: setup notes for agents

This folder adds a WARDEN tab to [boring.notch](https://github.com/TheBoredTeam/boring.notch), the open source notch app for macOS. The tab runs WARDEN's own HUD bundle (the same globes as the menu-bar HUD), peeks the notch open when an agent needs the user, and can start WARDEN when it is not running.

boring.notch has no plugin API, so the integration is a patch series on top of upstream, not a separate package. `patches/` holds 8 commits cut from upstream commit `99900bf` (2026-08-29). The patches are GPL-3.0, the license of boring.notch. The rest of this repo is not.

## Set it up

Needs macOS with a notch-capable display (or boring.notch's non-notch mode), full Xcode (not only the command line tools), and WARDEN installed with bundle id `com.warden.app`.

```sh
integrations/boring-notch/setup.sh              # clone, patch, build, install to /Applications
integrations/boring-notch/setup.sh --no-install # clone and patch only
```

The clone lands in `../boring.notch` next to this repo unless you pass a path. The build replaces `/Applications/boringNotch.app` and relaunches it: ask the user first if they already run boring.notch.

Then open the notch and pick the WARDEN tab. With WARDEN running the globes appear. Without it the tab shows a Start WARDEN button.

One step only a person can do: if Xcode's license was never accepted, the build script stops and prints the two `sudo` commands to run. Do not run them for the user.

## If a patch does not apply

Upstream moved. Check out `99900bf` (the script does this), apply there, then rebase the `warden-section` branch onto the new upstream and fix conflicts by hand. The touch points in upstream files are small: `NotchViews` enum (`.warden` case), `ContentView` tab switch, `TabSelectionView`, `SettingsView`, `boringNotchApp.swift`, `Constants.swift`, the entitlements, and the Xcode project. The new code lives in `boringNotch/components/Warden/`. After a rebase, regenerate the patches from the clone:

```sh
git format-patch --no-signature -o <this folder>/patches origin/main..warden-section
```

and update `BASE` in `setup.sh` to the new upstream commit.

## How the two sides talk

| Piece | Where | What it does |
|---|---|---|
| Handshake file | `~/.warden/bridge.json`, mode 0600 | WARDEN writes `{port, token}` at each launch. The notch reads it and watches it for changes. |
| Loopback bridge | `src-tauri/src/bridge.rs` | HTTP on `127.0.0.1` only, token checked in constant time, foreign origins refused. Serves `/hud.html`, the HUD's commands and events, and `/attention`. |
| Frontend transport | `web/viz/shared/state/hudTransport.ts` | The HUD picks Tauri IPC or the HTTP bridge at runtime. No view code knows which host it is in. |
| Section view | `WardenSectionView.swift` | A `WKWebView` on `http://127.0.0.1:<port>/hud.html`. The token goes in as `window.__WARDEN_BRIDGE__` at document start, never in the URL. Calls `window.__wardenSetVisible(bool)` so the canvas stops rendering when the tab is off screen. The page posts to the `wardenHost` message handler to ask the notch to close after a globe click. |
| Attention monitor | `WardenAttentionMonitor.swift` | Holds a stream on `/attention` and peeks the notch when an agent stops on the user. The open connection tells WARDEN to stay quiet, so only one of them alerts. Toggle: `wardenAttentionAlerts` in settings. |
| Launcher | `WardenLauncher.swift` | Starts WARDEN through `NSWorkspace` by bundle id, not a spawned `open`, and counts it as up only when the handshake file appears. |

## Gotchas that cost time

- The sandbox entitlement `temporary-exception.files.home-relative-path.read-only` for `/.warden/` is required. Without it the sandbox hands back the app container and the handshake file is never found.
- The XPC helper must not be sandboxed or the Accessibility prompt is a dead button. `build-and-install.sh` checks this and stops if it is.
- Hardened runtime is off in the ad-hoc build only: with no team, library validation would refuse the bundled frameworks and the app dies before `main`.
- The build unregisters the copy under `.build/` so Launchpad and Spotlight show one boringNotch, not two.
- The tab body is built only while the notch is open, so the web view is cached across opens rather than rebooted on every glance.
