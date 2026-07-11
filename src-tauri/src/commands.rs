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
