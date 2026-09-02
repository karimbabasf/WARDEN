use crate::observe::{
    BindMode, EventSink, FrameSource, GrantStore, HostShare, ObserveEvent, ObserverHub,
};
use crate::radar::{self, RadarState};
use crate::store::Store;
use crate::util::{default_claude_sessions_dir, default_db_path};
use anyhow::Result;
use std::sync::{Arc, RwLock};

/// Shared app state: the SQLite store plus the last-pushed RADAR forest cache.
/// Cloned into Tauri's managed state and the background watchers.
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub radar_state: crate::scheduler::RadarStateCache,
    pub observe: ObserveRuntime,
}

/// The remote-observation runtime.
///
/// The `tauri::AppHandle` lives HERE, not inside `observe/`. That module is handed two
/// closures (a frame source and an event sink) and nothing else, so it has no route to a
/// Tauri command even if a bug reached for one, and no way to name the un-redacted radar
/// types. This struct is the seam where those two capabilities are granted, deliberately
/// narrow and in one place.
#[derive(Clone)]
pub struct ObserveRuntime {
    pub host: HostShare,
    pub hub: ObserverHub,
    app: Arc<RwLock<Option<tauri::AppHandle>>>,
}

impl ObserveRuntime {
    fn new(store: Store, radar_state: crate::scheduler::RadarStateCache) -> Self {
        let app: Arc<RwLock<Option<tauri::AppHandle>>> = Arc::new(RwLock::new(None));

        // The one path from the radar to the wire. `project_state` is applied HERE, so what
        // the transport receives is already redacted and it never sees a `RadarState`.
        let cache = radar_state.clone();
        let frames: FrameSource = Arc::new(move |salt: &[u8; 16]| {
            crate::scheduler::latest_cached_radar_state(&cache).map(|radar| {
                crate::observe::project_state(&radar, crate::observe::Profile::Shapes, salt)
            })
        });

        let sink_app = app.clone();
        let events: EventSink = Arc::new(move |event: ObserveEvent| {
            use tauri::Emitter;
            let Ok(guard) = sink_app.read() else { return };
            let Some(app) = guard.as_ref() else { return };
            let _ = match event {
                ObserveEvent::Grants => app.emit("observe:grants", ()),
                ObserveEvent::Peers => app.emit("observe:peers", ()),
                ObserveEvent::Frame { peer_id } => {
                    app.emit("observe:frame", serde_json::json!({ "peerId": peer_id }))
                }
                ObserveEvent::Approval(a) => app.emit("observe:approval", a),
            };
        });

        let host_label = hostname_label();
        Self {
            host: HostShare::new(
                GrantStore::new(store.clone()),
                frames,
                events.clone(),
                host_label,
                crate::util::observe_key_path(),
                BindMode::Public,
            ),
            hub: ObserverHub::new(store, events, BindMode::Public),
            app,
        }
    }

    /// Hand the runtime its event channel once Tauri has one. Until this is called the
    /// sink is a no-op, which is the correct behaviour during startup.
    pub fn attach_app(&self, app: tauri::AppHandle) {
        if let Ok(mut slot) = self.app.write() {
            *slot = Some(app);
        }
    }
}

/// A short, human-meaningful name for this machine, shown to an observer as the host
/// label. Falls back to a generic string rather than leaking a username.
fn hostname_label() -> String {
    std::env::var("WARDEN_HOST_LABEL").unwrap_or_else(|_| "Warden host".to_string())
}

impl AppState {
    pub fn init() -> Result<Self> {
        let store = Store::open(default_db_path())?;
        Ok(Self::from_store(store))
    }

    fn from_store(store: Store) -> Self {
        let radar_state = crate::scheduler::new_radar_state_cache();
        let observe = ObserveRuntime::new(store.clone(), radar_state.clone());
        Self {
            store,
            radar_state,
            observe,
        }
    }

    #[cfg(test)]
    pub(crate) fn from_store_for_test(store: Store) -> Self {
        Self::from_store(store)
    }

    pub fn cache_radar_state(&self, radar: RadarState) {
        crate::scheduler::cache_radar_state(&self.radar_state, radar);
    }

    pub fn cached_radar_state(&self) -> Option<RadarState> {
        crate::scheduler::latest_cached_radar_state(&self.radar_state)
    }
}

/// Diagnostic sink: the packaged window has no devtools, so live webview JS can
/// report where the R3F island breaks by invoking this and reading stderr.
#[tauri::command]
pub fn diag(msg: String) {
    eprintln!("[WARDEN-DIAG] {msg}");
}

/// Hide the WARDEN window. The daemon keeps running and the window is
/// re-summonable via the tray menu or the global hotkey. Best-effort: a missing
/// window must not error the frontend, so failures are swallowed.
#[tauri::command]
pub async fn hide_overlay(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
    Ok(())
}

/// Minimize the WARDEN window to the Dock (macOS). Best-effort: a missing window
/// must not error the frontend, so failures are swallowed.
#[tauri::command]
pub async fn minimize_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.minimize();
    }
    Ok(())
}

/// Hide the WARDEN window. The daemon keeps running and the window stays
/// re-summonable via the tray menu or the global hotkey. Best-effort: a missing
/// window must not error the frontend, so failures are swallowed.
#[tauri::command]
pub async fn hide_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
    }
    Ok(())
}

/// Hide the menu-bar HUD.
///
/// Called by the HUD itself, and only once its collapse has finished playing: hiding
/// the window is what ENDS the animation, so Rust must not do it on the click. The tray
/// asks for a dismiss, the frontend runs the genie, and this is the last step.
/// Best-effort: a missing window must not error the frontend.
#[tauri::command]
pub async fn hud_hide(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("hud") {
        let _ = w.hide();
    }
    Ok(())
}

/// Take me to this agent: raise the war room and select the globe the HUD was showing.
///
/// The HUD's one and only control. It raises the window BEFORE the HUD finishes
/// collapsing, deliberately, closing first would leave a beat of empty desktop between
/// the click and the window it summoned.
#[tauri::command]
pub async fn hud_focus_agent(app: tauri::AppHandle, agent_id: String) -> Result<(), String> {
    use tauri::Emitter;
    // OPEN WARDEN, not just "show its window": on macOS those are two different acts
    // and doing only the second one is what made this feel like a no-op. See
    // `raise_overlay`. The event is emitted whether or not the window was there, so a
    // war room that mounts a moment later still lands on the right globe.
    crate::raise_overlay(&app);
    let _ = app.emit_to(
        "overlay",
        "warden_focus_agent",
        serde_json::json!({ "agentId": agent_id }),
    );
    Ok(())
}

/// RADAR: the live agent forest, contract-shaped (`radar_state`). Reads live
/// transcript tails before returning so the frontend's visible-RADAR polling
/// closes any missed filesystem-event gap, then caches the fresh forest for push
/// consumers.
#[tauri::command]
pub async fn get_radar_state(state: tauri::State<'_, AppState>) -> Result<RadarState, String> {
    Ok(fresh_radar_state_for_read(&state))
}

/// The longest session name WARDEN will store or write. Keeps a pathological paste out
/// of both the SQLite row and the transcript line.
const MAX_SESSION_NAME: usize = 120;

/// Reject anything that would corrupt the JSONL transcript or the UI: newlines would
/// split one record into two (the parser is line-delimited), and other control
/// characters render as garbage. Returns the cleaned name.
fn clean_session_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("name is empty".into());
    }
    if name.chars().any(|c| c.is_control()) {
        return Err("name contains control characters".into());
    }
    Ok(crate::util::truncate_chars(name, MAX_SESSION_NAME))
}

/// Rename a session as WARDEN displays it, and (for Claude) push that name back to the
/// harness so its own UI agrees.
///
/// Two layers on purpose:
/// 1. `warden_display_name` in the session's `meta_json` is WARDEN's own record. It is
///    authoritative for the radar label, applies instantly, and survives re-ingest
///    because the merge only overwrites keys the incoming batch actually carries.
/// 2. For Claude, a `custom-title` record is APPENDED to the transcript. That is exactly
///    how Claude itself records a title (the current title is the last such record), so a
///    single atomic append is a well-formed edit rather than a rewrite. Nothing is ever
///    truncated or modified in place, and no other harness is written to at all.
///
/// This is the one place WARDEN writes outside its own database. It writes only to the
/// harness's session metadata, never to a watched project's files.
#[tauri::command]
pub async fn rename_session(
    state: tauri::State<'_, AppState>,
    agent_id: String,
    name: String,
) -> Result<String, String> {
    let name = clean_session_name(&name)?;

    state
        .store
        .merge_session_meta(&agent_id, &serde_json::json!({ "warden_display_name": name }))
        .map_err(|e| format!("store rename: {e}"))?;

    // Best-effort push to Claude. A failure here must not lose the rename: layer 1 has
    // already committed, so the radar shows the new name regardless.
    if let Ok(sessions) = state.store.sessions() {
        if let Some(s) = sessions.iter().find(|s| s.id == agent_id) {
            if matches!(s.harness, crate::ir::Harness::ClaudeCode) {
                let root = crate::util::default_claude_projects();
                if let Err(e) =
                    append_claude_custom_title(&root, &s.source_path, &s.external_id, &name)
                {
                    eprintln!("[WARDEN] rename stored, transcript push skipped: {e}");
                }
            }
        }
    }
    Ok(name)
}

/// Append one `custom-title` record to a Claude transcript.
///
/// Safety rules, all load-bearing:
/// * the target must be an existing `.jsonl` file under the Claude projects root, so a
///   crafted id cannot make WARDEN append to an arbitrary file,
/// * opened `O_APPEND`, one `write` of a single complete line. A lone append under
///   `PIPE_BUF` cannot interleave with the harness's own appends,
/// * nothing is read, truncated, or rewritten, so a concurrent writer cannot lose data.
fn append_claude_custom_title(
    root: &std::path::Path,
    source_path: &std::path::Path,
    external_id: &str,
    name: &str,
) -> Result<(), String> {
    use std::io::Write;

    let canonical_root = root.canonicalize().map_err(|e| format!("claude root: {e}"))?;
    let canonical = source_path
        .canonicalize()
        .map_err(|e| format!("transcript: {e}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("transcript is outside the Claude projects root".into());
    }
    if canonical.extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return Err("transcript is not a .jsonl file".into());
    }

    // serde_json does the escaping, so a name with quotes or backslashes stays valid.
    let line = serde_json::to_string(&serde_json::json!({
        "type": "custom-title",
        "customTitle": name,
        "sessionId": external_id,
    }))
    .map_err(|e| format!("encode: {e}"))?;

    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&canonical)
        .map_err(|e| format!("open: {e}"))?;
    f.write_all(format!("{line}\n").as_bytes())
        .map_err(|e| format!("append: {e}"))?;
    Ok(())
}

/// Reveal a file in Finder, selecting it in its containing folder.
///
/// Takes the `~`-folded DISPLAY path carried on the radar state, so the absolute path
/// never has to live in frontend state (and therefore can never be transmitted to a
/// remote observer). Expands and canonicalizes it, and refuses anything that does not
/// resolve to a real existing path.
#[tauri::command]
pub async fn reveal_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;

    let expanded = crate::util::expand_tilde(&path);
    let canonical = expanded
        .canonicalize()
        .map_err(|e| format!("no such path: {e}"))?;
    app.opener()
        .reveal_item_in_dir(&canonical)
        .map_err(|e| format!("reveal: {e}"))
}

/// Resolve one agent, then hand both terminal commands the same answer.
///
/// Both the probe and the act go through here, so the button's claim and the
/// click's behaviour can never disagree: they read different halves of one
/// resolution rather than each deciding for themselves.
fn locate_agent_terminal(state: &AppState, agent_id: &str) -> crate::terminal::Located {
    let Some(radar) = state.cached_radar_state() else {
        return crate::terminal::Located {
            target: crate::terminal::TerminalTarget {
                reachable: false,
                app: None,
                via_agent_id: None,
                via_label: None,
                reason: Some("the radar has not reported yet".into()),
            },
            focus: None,
        };
    };
    let registry =
        crate::radar::liveness::read_claude_registry(&crate::util::default_claude_sessions_dir());

    crate::terminal::locate(
        &radar.agents,
        |id| state.store.session_by_id(id).ok().flatten(),
        &registry,
        agent_id,
        crate::platform::process_alive,
        crate::platform::controlling_tty,
        crate::platform::terminal_host_for_pid,
    )
}

/// "Take me there": can WARDEN raise the terminal window this agent is in?
///
/// Called when the panel opens an agent, NOT on every radar frame: it shells out
/// to `ps` twice. It sends no Apple event, so it can never raise a permission
/// dialog on its own; consent is asked for by the click, with the reason on
/// screen.
#[tauri::command]
pub async fn agent_terminal_target(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> Result<crate::terminal::TerminalTarget, String> {
    Ok(locate_agent_terminal(&state, &agent_id).target)
}

/// Select the tab this agent is running in and bring its window to the front.
///
/// Read-and-raise only: see `platform/macos.rs`. Nothing is typed into the
/// session, so this stays inside the read-only posture rather than reopening the
/// exception the armed-compaction feature needed.
///
/// The resolution is redone here rather than trusted from the probe. A window
/// can close between the panel opening and the click, and a stale tty would
/// otherwise raise whichever tab inherited that device number.
///
/// A miss comes back as `ok: false` rather than as an `Err`, and `denied` is
/// flagged as its own field. The one permanent failure (macOS recorded a refusal
/// and will never ask again) needs a different offer from every other one, and
/// the frontend must not have to recognise it by matching English in a message.
#[tauri::command]
pub async fn focus_agent_terminal(
    state: tauri::State<'_, AppState>,
    agent_id: String,
) -> Result<FocusOutcome, String> {
    let located = locate_agent_terminal(&state, &agent_id);
    let Some((app, tty)) = located.focus else {
        return Ok(FocusOutcome::failed(
            located
                .target
                .reason
                .unwrap_or_else(|| "WARDEN cannot find that window".into()),
            false,
        ));
    };
    match crate::platform::focus_tty(app, &tty) {
        Ok(()) => Ok(FocusOutcome {
            ok: true,
            denied: false,
            message: None,
        }),
        Err(e) => {
            let denied = matches!(e, crate::platform::AutomationError::NotPermitted);
            Ok(FocusOutcome::failed(e.to_string(), denied))
        }
    }
}

/// What a "take me there" click did. `ok` is the only success signal; `denied`
/// separates the sticky permission refusal from every other failure.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusOutcome {
    pub ok: bool,
    pub denied: bool,
    pub message: Option<String>,
}

impl FocusOutcome {
    fn failed(message: String, denied: bool) -> Self {
        Self {
            ok: false,
            denied,
            message: Some(message),
        }
    }
}

/// Open Privacy and Security, Automation.
///
/// A denial there is sticky and never re-prompts, so a "take me there" that was
/// once refused can only be recovered from this pane. The panel offers it as the
/// next step after that specific failure, never on its own.
#[tauri::command]
pub async fn open_automation_settings() -> Result<(), String> {
    crate::platform::open_automation_settings();
    Ok(())
}

/// A bounded peek at a file an agent is actually touching, for the detail panel's
/// "show me what it is reading/writing" control.
///
/// SECURITY: this is deliberately NOT a general file-read primitive. The requested
/// path must already appear as a target on the CURRENT radar state (an in-flight
/// action's target, or a row in some agent's recent activity). Anything else is
/// refused, so a compromised webview cannot turn this into arbitrary local file
/// read. The allowlist is rebuilt per call from live state rather than cached, so
/// revoking is automatic: once a path ages out of the feed it stops being readable.
///
/// It is also unreachable from the network by construction. Commands are invoked
/// only by the local webview; a remote observer receives `ObservedState`, which
/// carries no paths at all, so an observer has nothing to pass here in the first
/// place.
///
/// Bounded on BOTH axes (bytes and lines) because the point is a glance, not a
/// viewer, and because a multi-megabyte file would otherwise cross the IPC bridge.
const PREVIEW_MAX_BYTES: usize = 64 * 1024;
const PREVIEW_MAX_LINES: usize = 400;

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    /// Echoed back `~`-folded, so the caller never learns an absolute path.
    pub path: String,
    pub text: String,
    /// True when the file was longer than the byte or line budget.
    pub truncated: bool,
    pub total_bytes: u64,
    pub shown_lines: usize,
    /// False when the bytes are not valid UTF-8 (a binary); `text` is then empty.
    pub is_text: bool,
}

#[tauri::command]
pub async fn preview_file(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<FilePreview, String> {
    use std::io::Read;

    let radar = fresh_radar_state_for_read(&state);
    if !radar_mentions_path(&radar, &path) {
        return Err("that path is not on the radar right now".into());
    }

    let expanded = crate::util::expand_tilde(&path);
    let canonical = expanded
        .canonicalize()
        .map_err(|e| format!("no such path: {e}"))?;

    let meta = std::fs::metadata(&canonical).map_err(|e| format!("stat: {e}"))?;
    if meta.is_dir() {
        return Err("that path is a directory".into());
    }
    let total_bytes = meta.len();

    let mut file = std::fs::File::open(&canonical).map_err(|e| format!("open: {e}"))?;
    let mut buf = vec![0u8; PREVIEW_MAX_BYTES];
    let read = file.read(&mut buf).map_err(|e| format!("read: {e}"))?;
    buf.truncate(read);

    let Ok(whole) = String::from_utf8(buf) else {
        return Ok(FilePreview {
            path,
            text: String::new(),
            truncated: total_bytes > read as u64,
            total_bytes,
            shown_lines: 0,
            is_text: false,
        });
    };

    let mut text = String::new();
    let mut shown_lines = 0usize;
    for line in whole.lines().take(PREVIEW_MAX_LINES) {
        text.push_str(line);
        text.push('\n');
        shown_lines += 1;
    }

    Ok(FilePreview {
        path,
        truncated: total_bytes > read as u64 || whole.lines().count() > shown_lines,
        text,
        total_bytes,
        shown_lines,
        is_text: true,
    })
}

/// Is this display path something the live radar is actually pointing at?
///
/// Compares against the exact strings the frontend was given, which is the whole
/// point: the caller can only ask for what it was already shown.
fn radar_mentions_path(radar: &RadarState, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    radar.agents.iter().any(|a| {
        a.current_action
            .as_ref()
            .and_then(|act| act.target.as_deref())
            == Some(path)
            || a.recent_activity
                .iter()
                .any(|row| row.target.as_deref() == Some(path))
    })
}

/// Exactly what a remote observer would see of this machine right now.
///
/// Runs the live radar through the SAME [`crate::observe::project_state`] the wire uses.
/// There is deliberately no second "preview formatter": a preview that can drift from the
/// wire is worse than no preview, because it manufactures false confidence right at the
/// moment the user decides whether to share.
///
/// `salt` is per-grant in real use; the preview uses a fixed salt because the hashes are
/// not the point here, the absence of names and paths is.
#[tauri::command]
pub async fn preview_observed_state(
    state: tauri::State<'_, AppState>,
) -> Result<crate::observe::ObservedState, String> {
    let radar = fresh_radar_state_for_read(&state);
    Ok(crate::observe::project_state(
        &radar,
        crate::observe::Profile::Shapes,
        &[0u8; 16],
    ))
}

fn fresh_radar_state_for_read(state: &AppState) -> RadarState {
    fresh_radar_state_for_read_with(state, &crate::platform::process_index())
}

/// [`fresh_radar_state_for_read`] with the process table INJECTED, so a test can
/// drive the read path against fixture transcripts without depending on which
/// harnesses happen to be running on the machine executing the suite.
fn fresh_radar_state_for_read_with(
    state: &AppState,
    procs: &crate::radar::procs::ProcessIndex,
) -> RadarState {
    let sessions_dir = default_claude_sessions_dir();
    radar::refresh_live_context(&state.store, &sessions_dir);
    let radar = radar::recompute_radar_state_with(&state.store, &sessions_dir, procs);
    state.cache_radar_state(radar.clone());
    radar
}

// ---------------------------------------------------------------------------
// Remote observation. Host side: sharing this machine.
// ---------------------------------------------------------------------------

/// A freshly minted grant. `token` is shown ONCE and is never retrievable again: the host
/// stores only a hash of it, so there is nothing to re-read.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewGrant {
    pub grant_id: String,
    pub token: String,
    pub expires_at: String,
}

/// One grant as the management tab renders it. Carries no secret and no verifier.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantRow {
    pub grant_id: String,
    pub label: String,
    pub state: String,
    pub created_at: String,
    pub expires_at: String,
    pub redeemed_fingerprint: Option<String>,
    pub last_seen_at: Option<String>,
    pub connected: bool,
}

/// Bind the observation endpoint and start accepting observers.
///
/// This is the moment Warden first touches the network. Before it, the app binds no
/// socket and contacts no relay, which is what keeps "fully local by default" literally
/// true. Idempotent: calling it while already sharing returns the same identity.
#[tauri::command]
pub async fn observe_start_sharing(
    state: tauri::State<'_, AppState>,
) -> Result<crate::observe::HostIdentity, String> {
    state
        .observe
        .host
        .start()
        .await
        .map_err(|e| format!("{e:#}"))
}

/// Stop sharing: unbind the endpoint and close every live observer.
#[tauri::command]
pub async fn observe_stop_sharing(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.observe.host.stop().await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn observe_sharing_status(
    state: tauri::State<'_, AppState>,
) -> Result<crate::observe::SharingStatus, String> {
    Ok(state.observe.host.status().await)
}

/// Mint a single-use token for one friend.
///
/// Requires sharing to be on, because the token embeds this host's endpoint id and there
/// is no endpoint until then. The returned token is the only copy that will ever exist.
#[tauri::command]
pub async fn observe_create_grant(
    state: tauri::State<'_, AppState>,
    label: String,
    ttl_secs: u64,
) -> Result<NewGrant, String> {
    let endpoint_id = state
        .observe
        .host
        .endpoint_id()
        .await
        .ok_or("start sharing before creating a grant")?;
    let (record, token) = state
        .observe
        .host
        .grants()
        .create(
            endpoint_id,
            &label,
            ttl_secs,
            crate::observe::Profile::Shapes,
            chrono::Utc::now().timestamp(),
        )
        .map_err(|e| format!("{e:#}"))?;
    Ok(NewGrant {
        grant_id: record.grant_id,
        token,
        expires_at: stamp(record.expires_at),
    })
}

#[tauri::command]
pub async fn observe_list_grants(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<GrantRow>, String> {
    let host = &state.observe.host;
    // Flip anything stale before listing, so the tab never shows a pending grant that can
    // no longer be redeemed. Cosmetic only: redemption re-checks expiry regardless.
    let _ = host.grants().sweep_expired(chrono::Utc::now().timestamp());
    let rows = host.grants().list().map_err(|e| format!("{e:#}"))?;
    Ok(rows
        .into_iter()
        .map(|g| GrantRow {
            grant_id: g.grant_id,
            label: g.label,
            state: g.state.as_str().to_string(),
            created_at: stamp(g.created_at),
            expires_at: stamp(g.expires_at),
            redeemed_fingerprint: g
                .redeemed_by
                .as_ref()
                .map(crate::observe::transport::fingerprint),
            last_seen_at: g
                .last_seen_at
                .and_then(crate::observe::peers::unix_to_rfc3339),
            connected: g
                .redeemed_by
                .as_ref()
                .is_some_and(|id| host.is_connected(id)),
        })
        .collect())
}

/// Revoke a grant: mark the row, drop the endpoint from the allow set, and close the live
/// connection. All three, because the row alone races an in-flight frame.
#[tauri::command]
pub async fn observe_revoke_grant(
    state: tauri::State<'_, AppState>,
    grant_id: String,
) -> Result<(), String> {
    state
        .observe
        .host
        .revoke(&grant_id)
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn observe_pending_approvals(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::observe::PendingApproval>, String> {
    Ok(state.observe.host.pending_approvals())
}

/// Approve or deny a waiting observer. Nothing has been sent to them before this resolves,
/// which is what makes an intercepted token survivable.
#[tauri::command]
pub async fn observe_resolve_approval(
    state: tauri::State<'_, AppState>,
    conn_id: String,
    approve: bool,
) -> Result<(), String> {
    state.observe.host.resolve_approval(&conn_id, approve);
    Ok(())
}

// ---------------------------------------------------------------------------
// Remote observation. Observer side: watching someone else.
// ---------------------------------------------------------------------------

/// Redeem a token and start watching that host.
#[tauri::command]
pub async fn observe_add_peer(
    state: tauri::State<'_, AppState>,
    token: String,
) -> Result<crate::observe::PeerRow, String> {
    state
        .observe
        .hub
        .add_peer(&token)
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn observe_list_peers(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::observe::PeerRow>, String> {
    state.observe.hub.list().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn observe_remove_peer(
    state: tauri::State<'_, AppState>,
    peer_id: String,
) -> Result<(), String> {
    state
        .observe
        .hub
        .remove_peer(&peer_id)
        .map_err(|e| format!("{e:#}"))
}

/// The latest frame from one host, or `null` until the first one lands.
#[tauri::command]
pub async fn observe_peer_state(
    state: tauri::State<'_, AppState>,
    peer_id: String,
) -> Result<Option<crate::observe::ObservedState>, String> {
    state
        .observe
        .hub
        .peer_state(&peer_id)
        .map_err(|e| format!("{e:#}"))
}

/// Unix seconds to rfc3339, the shape every timestamp crosses IPC in.
fn stamp(ts: i64) -> String {
    crate::observe::peers::unix_to_rfc3339(ts).unwrap_or_default()
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn rejects_names_that_would_corrupt_a_line_delimited_transcript() {
        // A newline would split one JSONL record into two, which is the failure that
        // would actually damage a user's transcript.
        assert!(clean_session_name("a\nb").is_err());
        assert!(clean_session_name("a\r\nb").is_err());
        assert!(clean_session_name("tab\there").is_err());
        assert!(clean_session_name("null\0byte").is_err());
        assert!(clean_session_name("").is_err());
        assert!(clean_session_name("   ").is_err());
    }

    #[test]
    fn trims_and_caps_length() {
        assert_eq!(clean_session_name("  hello  ").expect("valid"), "hello");
        let long = "x".repeat(500);
        let cleaned = clean_session_name(&long).expect("valid");
        assert_eq!(cleaned.chars().count(), MAX_SESSION_NAME);
    }

    #[test]
    fn keeps_unicode_and_punctuation() {
        assert_eq!(
            clean_session_name("Warden: radar refactor (v2) 🚀").expect("valid"),
            "Warden: radar refactor (v2) 🚀"
        );
    }

    /// The write path: appends one well-formed record and leaves prior bytes untouched.
    #[test]
    fn appends_one_record_without_touching_existing_lines() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let proj = root.join("-Users-someone-repo");
        std::fs::create_dir_all(&proj).expect("mkdir");
        let file = proj.join("abc.jsonl");
        let original = "{\"type\":\"user\",\"uuid\":\"1\"}\n";
        std::fs::write(&file, original).expect("seed");

        append_claude_custom_title(root, &file, "abc", "New Name").expect("append succeeds");

        let mut got = String::new();
        std::fs::File::open(&file)
            .expect("open")
            .read_to_string(&mut got)
            .expect("read");
        assert!(got.starts_with(original), "existing bytes must be preserved");

        let last = got.lines().next_back().expect("a last line");
        let v: serde_json::Value = serde_json::from_str(last).expect("appended line is valid json");
        assert_eq!(v["type"], "custom-title");
        assert_eq!(v["customTitle"], "New Name");
        assert_eq!(v["sessionId"], "abc");
        assert!(got.ends_with('\n'), "must stay newline-terminated");
    }

    #[test]
    fn quotes_and_backslashes_survive_as_valid_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let file = root.join("x.jsonl");
        std::fs::write(&file, "").expect("seed");
        let tricky = r#"He said "hi" \ back"#;
        append_claude_custom_title(root, &file, "s", tricky).expect("append");
        let got = std::fs::read_to_string(&file).expect("read");
        let v: serde_json::Value =
            serde_json::from_str(got.trim_end()).expect("still valid json");
        assert_eq!(v["customTitle"], tricky);
    }

    /// A crafted id must not be able to steer the append outside the projects root.
    #[test]
    fn refuses_targets_outside_the_claude_projects_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("projects");
        std::fs::create_dir_all(&root).expect("mkdir");
        let outside = dir.path().join("elsewhere.jsonl");
        std::fs::write(&outside, "").expect("seed");

        let err = append_claude_custom_title(&root, &outside, "s", "n")
            .expect_err("must refuse a path outside the root");
        assert!(err.contains("outside"), "unexpected error: {err}");
        assert_eq!(
            std::fs::read_to_string(&outside).expect("read"),
            "",
            "the refused file must be untouched"
        );
    }

    #[test]
    fn refuses_a_non_jsonl_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let file = root.join("notes.md");
        std::fs::write(&file, "").expect("seed");
        let err = append_claude_custom_title(root, &file, "s", "n").expect_err("must refuse");
        assert!(err.contains("jsonl"), "unexpected error: {err}");
    }

    #[test]
    fn refuses_a_missing_file_rather_than_creating_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let missing = root.join("ghost.jsonl");
        assert!(append_claude_custom_title(root, &missing, "s", "n").is_err());
        assert!(!missing.exists(), "must not create a transcript");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Event, EventRecord, Harness, RawRef, Role, Session, Turn};
    use chrono::Utc;
    use serde_json::json;
    use std::path::PathBuf;

    fn seed_session(store: &Store, id: &str, harness: Harness, events: usize) {
        let now = Utc::now();
        let session = Session {
            id: id.into(),
            harness,
            external_id: id.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: now,
            meta: json!({}),
        };
        let turn = Turn {
            id: format!("{id}-t0"),
            session_id: id.into(),
            parent_id: None,
            role: Role::User,
            index: 0,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        let records = (0..events)
            .map(|i| EventRecord {
                id: format!("{id}-e{i}"),
                turn_id: turn.id.clone(),
                session_id: id.into(),
                ts: now,
                event: Event::UserPrompt {
                    text: format!("prompt {i}"),
                    attachments: vec![],
                    is_meta: false,
                },
                raw_ref: RawRef {
                    source_path: session.source_path.clone(),
                    offset: i as u64,
                    line: i as u32,
                },
            })
            .collect::<Vec<_>>();
        store
            .upsert_session_batch(&session, &[turn], &records, 0)
            .unwrap();
    }

    #[test]
    fn app_state_caches_latest_radar_state_for_read_commands() {
        let store = Store::memory().unwrap();
        let state = AppState::from_store_for_test(store);
        let radar = RadarState {
            generated_at: "2026-06-25T07:50:00Z".into(),
            agents: Vec::new(),
        };

        assert_eq!(state.cached_radar_state(), None);
        state.cache_radar_state(radar.clone());

        assert_eq!(
            state.cached_radar_state(),
            Some(radar),
            "RADAR read commands should return the last emitted forest without recomputing"
        );
    }

    #[test]
    fn radar_read_refreshes_live_codex_even_when_cache_is_stale() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let old_codex_sessions = std::env::var_os("WARDEN_CODEX_SESSIONS");
        let old_codex_archived = std::env::var_os("WARDEN_CODEX_ARCHIVED_SESSIONS");
        let old_claude_sessions = std::env::var_os("WARDEN_CLAUDE_SESSIONS");
        let old_claude_projects = std::env::var_os("WARDEN_CLAUDE_PROJECTS");

        let codex_sessions = tempfile::tempdir().unwrap();
        let codex_archived = tempfile::tempdir().unwrap();
        let claude_sessions = tempfile::tempdir().unwrap();
        let claude_projects = tempfile::tempdir().unwrap();
        std::env::set_var("WARDEN_CODEX_SESSIONS", codex_sessions.path());
        std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", codex_archived.path());
        std::env::set_var("WARDEN_CLAUDE_SESSIONS", claude_sessions.path());
        std::env::set_var("WARDEN_CLAUDE_PROJECTS", claude_projects.path());

        let live_dir = codex_sessions.path().join("2026/06/25");
        std::fs::create_dir_all(&live_dir).unwrap();
        let live =
            live_dir.join("rollout-2026-06-25T12-00-00-019f0040-0000-7000-8000-000000000001.jsonl");
        std::fs::write(
            &live,
            "{\"timestamp\":\"2026-06-25T19:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"019f0040-0000-7000-8000-000000000001\",\"cwd\":\"/tmp/LiveCodex\",\"model_provider\":\"openai\",\"originator\":\"Codex Desktop\",\"thread_source\":\"user\"}}\n\
             {\"timestamp\":\"2026-06-25T19:00:01Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"watch this live Codex agent\"}}\n",
        )
        .unwrap();

        let store = Store::memory().unwrap();
        let state = AppState::from_store_for_test(store);
        state.cache_radar_state(RadarState {
            generated_at: "2026-06-25T07:50:00Z".into(),
            agents: Vec::new(),
        });

        // Unscanned: this test is about the CACHE not hiding a live rollout, and
        // the fixture has no Codex process behind it. A real sweep would rightly
        // close it and the assertion would be testing the process rule instead.
        let radar =
            fresh_radar_state_for_read_with(&state, &crate::radar::procs::ProcessIndex::unscanned());

        assert!(
            radar
                .agents
                .iter()
                .any(|a| a.harness == "codex" && a.label == "LiveCodex"),
            "a RADAR read must not let a stale cache hide a live Codex rollout"
        );
        assert_eq!(
            state.store.watermark_offset(&live).unwrap(),
            std::fs::metadata(&live).unwrap().len(),
            "the read path must ingest the live Codex tail before returning"
        );

        match old_codex_sessions {
            Some(v) => std::env::set_var("WARDEN_CODEX_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_SESSIONS"),
        }
        match old_codex_archived {
            Some(v) => std::env::set_var("WARDEN_CODEX_ARCHIVED_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CODEX_ARCHIVED_SESSIONS"),
        }
        match old_claude_sessions {
            Some(v) => std::env::set_var("WARDEN_CLAUDE_SESSIONS", v),
            None => std::env::remove_var("WARDEN_CLAUDE_SESSIONS"),
        }
        match old_claude_projects {
            Some(v) => std::env::set_var("WARDEN_CLAUDE_PROJECTS", v),
            None => std::env::remove_var("WARDEN_CLAUDE_PROJECTS"),
        }
    }

    /// Smoke test for the RADAR core shape: with a seeded session, `assemble`
    /// returns a contract-shaped forest (one agent, serialized camelCase). Exercising
    /// the shared core avoids constructing a Tauri `State`.
    #[test]
    fn radar_command_core_returns_contract_shaped_forest() {
        let store = Store::memory().unwrap();
        seed_session(&store, "c1", Harness::ClaudeCode, 1);

        // Exercise the shared core (`assemble`) directly so no Tauri `State` is built.
        // A live Claude registry entry makes the seeded root OPEN under the membership
        // filter; is_alive=true and the codex predicate is unused for a Claude root.
        let reg = tempfile::tempdir().unwrap();
        std::fs::write(
            reg.path().join("100.json"),
            serde_json::json!({ "pid": 100, "sessionId": "c1", "cwd": "/work" }).to_string(),
        )
        .unwrap();
        let state = crate::radar::assemble(&store, reg.path(), &|_| true, &|_| false, &crate::radar::procs::ProcessIndex::unscanned(), Utc::now());
        assert_eq!(state.agents.len(), 1, "one seeded OPEN session yields one agent");
        let agent = &state.agents[0];
        assert_eq!(agent.id, "c1");
        assert_eq!(agent.harness, "claude_code");
        assert_eq!(agent.depth, 0);
        assert_eq!(agent.parent_id, None);

        let json = serde_json::to_string(&state).unwrap();
        for key in [
            "\"generatedAt\"",
            "\"fillPct\"",
            "\"contextTokens\"",
            "\"childCount\"",
        ] {
            assert!(json.contains(key), "contract camelCase key {key} present");
        }
    }
}
