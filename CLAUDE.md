# WARDEN: Project Guide

WARDEN is "the agent that watches your agents": a macOS **Tauri v2** app that tails your
local AI-coding transcripts (`~/.claude/projects`, `~/.codex/sessions`) and renders every
active and idle agent as a live 3D radar. It is local-first and has no API keys.

Two invariants are narrower than they used to be. Both are load-bearing, so read the exact
wording rather than the old summary:
- **Never writes to your projects.** WARDEN may write HARNESS SESSION METADATA (its own
  `sessions.meta_json`, and a `custom-title` line appended to a Claude transcript when you
  rename a session from the app). It must never write to a watched project's source files.
- **No network until you ask for one.** Remote observation binds its endpoint LAZILY, only
  when you press Share. At rest WARDEN opens no socket and contacts no relay. What crosses
  the wire is never `RadarState`: it is the redacted `ObservedState` projection.

## How we work in this repo
- **Delegate discovery.** For broad file search or multi-file reads, dispatch Explore or
  general-purpose subagents and keep only the conclusion. Do not inventory files in the
  main context.
- **Verify before claiming done.** Run the build and tests and read the real output.
  Evidence before assertions.
- **Read-only toward projects, always.** WARDEN never writes to your watched projects. The
  only writes outside its own DB are harness session metadata (see the invariants above),
  and they are confined to `commands.rs` so the write surface stays one file wide.
- **File reads are allowlisted, not open.** `preview_file` lets the panel show what an
  agent is reading or writing, but it refuses any path that is not already a target on
  the CURRENT radar state, so it can never become an arbitrary local-file-read
  primitive. The allowlist is rebuilt per call from live state, so it revokes itself as
  a path ages out of the feed. It is unreachable from the network by construction: a
  remote observer gets `ObservedState`, which carries no paths at all.
- **Redaction is a projection, not a scrub.** Anything an observer receives is built by
  `observe::project_state`. Never serialize a radar type toward the network, and never add
  a free-text field to `ObservedAgent` without deciding it may leave the machine: the
  canary test in `observe/projection.rs` is what enforces this and it must stay green.
- **Never `git push` or open a PR** without an explicit instruction in that message.
- Package manager is **pnpm**. Platform target: macOS Apple Silicon. OS-specific code is
  isolated in `platform/`, ready for future ports.

## Commands
| Goal | Command |
|---|---|
| Rust unit/golden tests | `cd src-tauri && cargo test` |
| Rust fast typecheck | `cd src-tauri && cargo check` |
| Rust lint (denies prod `unwrap()`) | `cd src-tauri && cargo clippy` |
| Frontend typecheck + bundle | `pnpm build` (= `tsc && vite build`) |
| Frontend tests | `pnpm test` (vitest) |
| Frontend import-boundary check | `pnpm check:arch` |
| Dev run (full app) | `pnpm tauri dev` |
| Full app bundle (real e2e gate, slow) | `pnpm tauri build` |
| Radar sandbox in a browser | `pnpm dev`, then open `/radar-lab.html` |

Toolchain pinned in `src-tauri/rust-toolchain.toml` (stable >= 1.85, for edition2024 deps).
Env is all OPTIONAL (see `.env.example`): `WARDEN_DB_PATH` plus the transcript-root
overrides `WARDEN_CLAUDE_PROJECTS` / `WARDEN_CODEX_SESSIONS`. No API keys: the radar is
fully local.

## Repo map
**Rust `src-tauri/src/`**: a single crate; `tauri` is confined to `lib.rs` / `commands.rs`.
Layered `ingest -> store -> radar -> commands/lib/scheduler`.
- `ir.rs` the canonical IR (single source of truth; every adapter maps raw records to this).
- `store.rs` rusqlite + FTS5 (sessions/turns/events/watermarks/radar_token_cache), byte-offset watermarks.
- `ingest/` the `Adapter` trait + `AdapterRegistry` + `claude_code.rs` / `codex.rs`. Adding a harness is one adapter, zero downstream changes.
- `radar.rs` + `radar/` the live agent forest: a façade over `model/assemble/agent/context/identity/live/status` + `awaiting` (the third globe state) + `composition/hierarchy/liveness` + `teams` (Claude agent-team rosters from `~/.claude/teams/*/config.json`, the source of real subagent names).
- `observe.rs` + `observe/` remote read-only observation: `projection` (the redaction boundary; the ONLY producer of wire data), `grants` (token codec + single-use/expiry), `transport` (iroh QUIC, host side), `peers` (observer side). Holds no `AppHandle` and cannot name a radar type, both asserted by tests.
- `scheduler.rs` + `scheduler/` the task drivers: `watch` (live-ingest) and `radar` (recompute + `RadarStateCache`). The recompute worker has TWO CPU guards and they do different jobs: serialization caps CONCURRENCY at one recompute, and `radar_min_interval()` (default 1s, `WARDEN_RADAR_MIN_INTERVAL_MS`) caps the RATE. Only the second one bounds a sustained stream: with the default zero debounce, an event landing while a recompute runs starts the next the instant it returns, so a live transcript tail used to pin a full core. The floor is leading-edge, so an isolated event still emits immediately.
- `terminal.rs` "take me there": resolves one agent to the terminal window it is running in (`agent id -> store session -> registry pid -> controlling tty -> the emulator tab owning that tty`), so the detail panel can raise it. A subagent has no process of its own, so the walk goes UP to its root and the answer names that root. Every hop can fail for a different real reason and the panel states which one, because a button that claims a window it cannot raise is worse than no button. READ-AND-RAISE ONLY: the AppleScript selects a tab and brings a window forward, and a test asserts the generated script contains no `do script` / `write text` / `keystroke`. That is what keeps "WARDEN writes into no other process" unconditional; do not reopen it.
- `util.rs` env + path helpers; `platform/` the OS seam (port + `macos.rs` + `fallback.rs`).
- `lib.rs` the Tauri builder / `setup()` (visible window on launch, tray, hotkey, startup backfill, watchers); `commands.rs` the `#[tauri::command]`s.

**Frontend `web/`**: an FSD-lite island; imports point DOWN only (`app -> views -> modules -> shared`, enforced by `pnpm check:arch`); the `@/` alias maps to `web/`.
- `index.html` (the `#war-room-root` mount); `main.ts` the Tauri event router; `style.css` the instrument tokens (see Design system below).
- `web/viz/`: `app/` (mount); `views/war-room/` (WarRoom + FleetRail + FilterBar + Breadcrumb); `modules/radar/` (the constellation, layout, detail panel, hover card, theme, peer constellation); `modules/observe/` (peers, grants UI, `observedToScene` adapter); `shared/{state,types,theme,scene,lib,ui}` (`bridge.ts` is the pure reducer in `state/`); `dev/preview/` (sandboxes).
- `web/fonts/` self-hosted WOFF2. The app opens no socket at rest, so a webfont CDN is not an option.

## Design system
Domain metaphor is air traffic control: a strip rack on the left, the scope in the
middle, the selected target's readout on the right.
- **Type**: Technor (display/UI) + Commit Mono (every numeral, path, and caps label,
  so readouts stay column-aligned). Both self-hosted and bundled. The face was
  chosen off a rendered specimen sheet at the app's real sizes, never from memory.
- **Colour**: the chrome is entirely neutral cold steel (`--bg #070910`, a
  `--surface-1/2/3` ramp, `--ink`/`--ink-soft`/`--ink-faint`). The ONLY hues are the
  two harness identities from `harnessColors.ts`, one red alert (`--danger`) used in
  exactly one place (a context gauge past 85%), and the AWAITING crimson `--alert`.
  Keeping the chrome colourless is what lets the constellation read as the hero.
- **A globe has THREE states, not two** (see "The third state" below). Working blazes
  and breathes; idle sits dim and steady; **awaiting strobes alert-red**. `--alert`
  (`#ff2740`) and `--alert-period` (`1.55s`) mirror `ALERT_HEX` / `ALERT_PERIOD` in
  `modules/radar/radarAlert.ts` by hand: the chip and the globe must flash on the same
  beat or the board reads as two alarms disagreeing. Change one, change both.
- **Depth is physical, never a glow**: a tone step, then a 1px border, then a 1px
  inset top highlight (`--lift`), then one tight key shadow (`--key` / `--key-lg`).
  There are no wide diffuse coloured glows and no `drop-shadow(0 0 Npx currentColor)`
  anywhere. That was the old phosphor-CRT look and it read as generated.
- **Motion**: easing tokens only (`--ease-out`, `--t-micro`/`--t-base`/`--t-travel`);
  never a bare CSS keyword and never one global duration. Animate transform and
  opacity. `prefers-reduced-motion` is honoured globally in CSS and per-component in
  the R3F scene.
- **Render rate has THREE states, not two** (`frameloopFor`): minimized is `never`,
  focused is `always`, and visible-but-blurred is `demand` paced by
  `BackgroundFrameTick` at `BACKGROUND_FPS` (30). Blurred is WARDEN's NORMAL state (it
  sits open behind the work it watches), and rendering it at display rate cost about
  half a core across the WebKit GPU process and WindowServer. Blurred is also the state
  the user LOOKS at most, so the paced rate is a legibility number, not only a cost
  one: 8fps read as broken. The pacer rides `requestAnimationFrame`, never
  `setInterval`, for both halves of that trade. Every tick lands on a real display
  refresh (an interval beats against the refresh and the drift reads as judder), and a
  fully occluded window stops getting rAF, so it pays nothing at all. Focus comes from Tauri's
  `onFocusChanged` through the bridge, never from `window.onblur` alone: as with
  `warden_hotkey`, the packaged app moves its window with native calls the webview does
  not see. Focus is a RATE input only and must never gate whether the radar updates. A
  Canvas on `demand` MUST mount `BackgroundFrameTick` or the scene freezes.
- **Two-rail layout**: "centre" means the FREE CHANNEL between the rails, not the
  viewport. Anything centred keys off `--rail-left-occupied` / `--rail-right-occupied`
  (`left: calc(50% + (L - R) / 2)`), and the camera gets the same insets in CSS
  pixels via `framingInsetLeft` / `framingInsetRight` so the constellation never
  frames underneath a panel. Two classes previously shared the name
  `.wd-radar-empty`; the full-screen one is now `.wd-radar-void`. Do not reuse a
  class name across a fixed-position overlay and an inline element.

## Dev harnesses (browser, no backend)
`pnpm dev`, then:
- `/radar-lab.html` the constellation + detail panel against a mock forest.
- `/war-room-lab.html` the WHOLE chrome, for LAYOUT passes. `?select=<n>` auto-clicks
  the nth fleet strip, so the both-rails-open state (the one that actually has to be
  checked for overlap) is reachable in a static screenshot.
Neither may redefine a design token locally: that is how a harness silently keeps
rendering a palette the app has already moved off.

## Conventions
- **Env helper**: `std::env::var("X").ok().map(...).unwrap_or_else(default)` (see `util.rs`).
- **IPC**: commands go web to Rust via `invoke`; events go Rust to web via `app.emit(name, json!{...})`.
- **Harness theme is one source of truth**: Claude is emerald, Codex is violet. Always pair colour with a glyph and label (color-blind a11y).
- **Adapter contract**: adding a harness is one adapter, zero downstream changes. An unknown record degrades gracefully; schema drift never drops a session.
- **Watermarks are byte-offset.** FSEvents coalesces rapid writes: on each event, seek to the saved offset and read to EOF; do not trust event counts.
- **Honest viz**: every globe and flare maps to a REAL signal (session liveness, context-token weight, subagent hierarchy). Never fabricate a count or a link. A sidecar that reports `spawnDepth >= 2` or names a `parentAgentId` was spawned by another SUBAGENT, so it must never fall back onto the root: the root is its ancestor, not its parent, and an unparented globe beats a wrong edge.
- **The third state: AWAITING (`radar/awaiting.rs`).** An agent stopped on the OPERATOR
  is neither working nor finished, and collapsing it into idle meant a blocked agent
  looked exactly like a done one. It is detected from three REAL signals, strongest
  first, and never inferred from silence:
  1. **The harness says so.** Claude's session registry vocabulary is
     `busy | shell | idle | waiting`, and `waiting` ships a `waitingFor` reason. That
     raw string stops at `AwaitingReason::from_registry`, which folds it into the closed
     `question | approval | input` set: `RadarAgent` is what the observer projection
     reads from, so free dialog text must not get that far. `shell` stays Idle (the human
     stepped out, the agent is not asking them anything).
  2. **A blocking tool is in flight.** `AskUserQuestion` / `ExitPlanMode` do not return
     until a human answers, so an unresolved one BEATS a Working verdict rather than
     being promoted from Idle. Scored at 126/126 recall on the local corpus
     (`cargo run --example awaiting_scan`).
  3. **A completed turn that ends on a question.** The only heuristic, and the only
     signal Codex has at all: 195 real rollouts contain zero approval-request records.
     It fires on 10% of completed turns locally, which is the rate agents genuinely end
     on "Want me to...?". Re-run `awaiting_scan` after touching it.
  Precedence is deliberate: a registry `busy` outranks a dangling question tool, because
  a false red is the one failure that teaches the operator to ignore the colour.
- **A trailing text block is NOT a finished turn.** The harness writes one transcript
  line per CONTENT BLOCK, so a mid-turn preamble ("Let me check the config.") and a final
  answer are both a lone text block and cannot be told apart by shape. 79% of text-only
  assistant lines are the preamble. Only the message's own `stop_reason` separates them,
  and it reaches the radar as `Event::AssistantText::turn_complete` (`None` means no
  information, so keep the old assumption). This matters most for SUBAGENTS and TEAM
  MEMBERS: a root has a PID and a live registry entry whose `status` is authoritative,
  they have neither, so this rule is their only judge. Score any change to it against
  real transcripts with `cargo run --example liveness_ab`.
- **The subagent sidecar is the source of nesting AND membership.** Every subagent of a session lands in one flat `<root>/subagents/` directory however deep it really is, so the path can never carry the tree. `parentAgentId` names the spawner, `spawnDepth` states the level, and `teamName` + `name` state team membership (the filename stopped carrying it: members are `agent-a<name>-<hex>`, not `agent-<name>@session-<hex>`). An in-process teammate has no `toolUseId` and its parent never logs a tool-result for it, so the 90s file-silence backstop must not apply to it: its lifecycle is the lead's, which `root_is_open` already enforces.
- **Position never depends on activity, with one exception: leaving.** A terminal agent stops holding a slot on the board the frame it goes terminal (`boardAgents`), so its siblings close ranks as it implodes rather than 5s later when the backend finally drops it. The filter lives INSIDE `layoutRadarScene` so every consumer still computes one identical board.
- **FSD layering (frontend)**: imports point DOWN only; no sibling-module imports; `dev/` is exempt. Use the `@/` alias; colocate tests; no app-wide barrels.
- **Rust module form**: `name.rs` + `name/` with a slim façade re-exporting a narrow public API (not a glob `pub use *`); cross-submodule internals are `pub(crate)`.
- **No production `unwrap()`**: clippy denies it (`unwrap_used = "deny"`); use `.expect("invariant")` for true invariants and `?` to propagate. Tests are exempt. `anyhow` everywhere.
- **Platform isolation**: all OS-specific runtime code lives in `platform/`; no `#[cfg(target_os)]` scattered elsewhere.

## External transcript layouts
- Claude: `~/.claude/projects/**/*.jsonl`.
- Codex: `~/.codex/sessions/YYYY/MM/DD/rollout-<ISO>-<uuid>.jsonl` (plus `~/.codex/archived_sessions/**`). Envelope: `{timestamp, type, payload}`.
