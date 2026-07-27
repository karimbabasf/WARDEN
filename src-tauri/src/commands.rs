use crate::radar::{self, RadarState};
use crate::store::Store;
use crate::util::{default_claude_sessions_dir, default_db_path};
use anyhow::Result;

/// Shared app state: the SQLite store plus the last-pushed RADAR forest cache.
/// Cloned into Tauri's managed state and the background watchers.
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub radar_state: crate::scheduler::RadarStateCache,
}
impl AppState {
    pub fn init() -> Result<Self> {
        let store = Store::open(default_db_path())?;
        Ok(Self::from_store(store))
    }

    fn from_store(store: Store) -> Self {
        Self {
            store,
            radar_state: crate::scheduler::new_radar_state_cache(),
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
fn fresh_radar_state_for_read(state: &AppState) -> RadarState {
    let sessions_dir = default_claude_sessions_dir();
    radar::refresh_live_context(&state.store, &sessions_dir);
    let radar = radar::recompute_radar_state(&state.store, &sessions_dir);
    state.cache_radar_state(radar.clone());
    radar
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

        let radar = fresh_radar_state_for_read(&state);

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
        let state = crate::radar::assemble(&store, reg.path(), &|_| true, &|_| false, Utc::now());
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
