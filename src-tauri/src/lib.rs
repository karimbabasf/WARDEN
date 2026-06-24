pub mod brain;
pub mod commands;
pub mod config;
pub mod detectors;
pub mod featurizer;
pub mod forge;
pub mod harness_theme;
pub mod ingest;
pub mod ir;
pub mod radar;
pub mod redaction;
pub mod scaffold;
pub mod scheduler;
pub mod store;
pub mod util;

use commands::*;
use tauri::menu::MenuBuilder;

/// Holds the RADAR liveness watchers so they outlive `setup()`. A distinct type
/// from `scheduler::WatcherGuard` because Tauri's managed state is keyed by type —
/// the ingest watchers and the radar watchers must each get their own slot.
struct RadarWatcherGuard(#[allow(dead_code)] std::sync::Mutex<Vec<notify::RecommendedWatcher>>);
use tauri::tray::TrayIconBuilder;
use tauri::{ActivationPolicy, Emitter, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Reveal the pre-warmed overlay: disable click-through, show, focus, and signal
/// the frontend to animate in. Idempotent — safe to call when already visible.
fn summon_overlay(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.set_ignore_cursor_events(false);
        let _ = w.show();
        let _ = w.set_focus();
        let _ = app.emit(
            "warden_hotkey",
            serde_json::json!({"hotkey":"cmd+shift+space"}),
        );
    }
}

/// Hide the overlay and restore desktop click-through (idle state). Shared by the
/// hotkey toggle and the `tauri://blur` dismissal.
fn dismiss_overlay(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
        let _ = w.set_ignore_cursor_events(true);
    }
}

pub fn run() {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            // 1) Become a menubar-only agent BEFORE any window is shown, else macOS
            //    flashes a Dock icon for the pre-warmed overlay. Paired with
            //    LSUIElement in Info.plist for the bundled .app.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Accessory);

            let state = AppState::init().map_err(|e| format!("state init: {e}"))?;

            // 2) Live FSEvents watchers (one per adapter root). Best-effort: a watch
            //    failure logs and never aborts startup — backfill + on-ask ingest
            //    still work. Watchers must outlive setup(), so park them in state.
            let watch_registry =
                std::sync::Arc::new(ingest::AdapterRegistry::new(state.store.clone()));
            match scheduler::spawn_watchers(
                watch_registry,
                state.store.clone(),
                app.handle().clone(),
            ) {
                Ok(watchers) => {
                    app.manage(scheduler::WatcherGuard::new(watchers));
                }
                Err(e) => tracing::warn!(error=%format!("{e:#}"), "live watchers failed to start"),
            }

            // 2b) RADAR liveness watchers: the Claude `~/.claude/sessions` registry
            //     (bloom/implode) + the Codex live/archived roots (archive-move =
            //     done). On any change the whole forest is recomputed and pushed as
            //     `radar_state`. Best-effort, same isolation as the ingest watchers.
            let radar_roots = vec![
                util::default_codex_sessions(),
                util::default_codex_archived_sessions(),
                util::default_claude_projects(),
            ];
            match scheduler::spawn_radar_watcher(
                state.store.clone(),
                util::default_claude_sessions_dir(),
                radar_roots,
                app.handle().clone(),
            ) {
                Ok(watchers) => {
                    app.manage(RadarWatcherGuard(std::sync::Mutex::new(watchers)));
                }
                Err(e) => {
                    tracing::warn!(error=%format!("{e:#}"), "radar watchers failed to start")
                }
            }

            // 3) One-shot startup backfill so the HUD has data immediately. Runs off
            //    the UI thread; a failing adapter is isolated inside backfill_all and
            //    only logs — it never stalls the others or aborts startup.
            {
                let store = state.store.clone();
                let app_handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let backfill_store = store.clone();
                    let summary = tokio::task::spawn_blocking(move || {
                        let registry = ingest::AdapterRegistry::new(backfill_store.clone());
                        registry.backfill_all(&backfill_store)
                    })
                    .await;
                    match summary {
                        Ok(summary) => {
                            for err in &summary.errors {
                                tracing::warn!(error = %err, "startup backfill: adapter failed (isolated)");
                            }
                            for (harness, sessions, events) in &summary.by_harness {
                                tracing::info!(
                                    harness = harness.as_str(),
                                    sessions,
                                    events,
                                    "startup backfill complete"
                                );
                            }
                            let ingested_sessions: usize = summary
                                .by_harness
                                .iter()
                                .map(|(_, sessions, _)| *sessions)
                                .sum();
                            let ingested_events: usize = summary
                                .by_harness
                                .iter()
                                .map(|(_, _, events)| *events)
                                .sum();
                            let by_harness = summary
                                .by_harness
                                .iter()
                                .map(|(harness, sessions, events)| {
                                    serde_json::json!({
                                        "harness": harness.as_str(),
                                        "sessions": sessions,
                                        "events": events,
                                    })
                                })
                                .collect::<Vec<_>>();
                            let (total_sessions, total_events, finding_count) =
                                store.counts().unwrap_or((0, 0, 0));
                            let _ = app_handle.emit(
                                "ingest_progress",
                                serde_json::json!({
                                    "phase": "startup_backfill",
                                    "status": "complete",
                                    "ingested_sessions": ingested_sessions,
                                    "ingested_events": ingested_events,
                                    "total_sessions": total_sessions,
                                    "total_events": total_events,
                                    "finding_count": finding_count,
                                    "by_harness": by_harness,
                                }),
                            );
                        }
                        Err(e) => tracing::warn!(error = %e, "startup backfill task panicked"),
                    }
                });
            }

            app.manage(state);

            // 4) Tray icon + menu (Summon / Status / Quit). The menu ids are matched
            //    by string in on_menu_event below.
            let menu = MenuBuilder::new(app)
                .text("summon", "Summon")
                .text("status", "Status")
                .separator()
                .text("quit", "Quit WARDEN")
                .build()?;
            let mut tray = TrayIconBuilder::with_id("warden-tray")
                .tooltip("WARDEN — the agent that watches your agents")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "summon" => summon_overlay(app),
                    "status" => summon_overlay(app),
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon().cloned() {
                tray = tray.icon(icon);
            }
            tray.build(app)?;

            // 5) Pre-warm the overlay: it is created hidden (tauri.conf.json
            //    visible:false). Start it click-through so the desktop stays
            //    interactive until summoned, and hide-on-blur to dismiss.
            if let Some(overlay) = app.get_webview_window("overlay") {
                let _ = overlay.set_ignore_cursor_events(true);
                let handle = app.handle().clone();
                overlay.on_window_event(move |event| {
                    if let WindowEvent::Focused(false) = event {
                        dismiss_overlay(&handle);
                    }
                });
            }

            // 6) Global hotkey ⌘⇧Space, replacing the old Alt+Space. Guard with
            //    is_registered so a hot-reload / re-setup does not double-register.
            let handle = app.handle().clone();
            let shortcut = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Space);
            let gs = app.global_shortcut();
            if !gs.is_registered(shortcut) {
                gs.on_shortcut(shortcut, move |_app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        // Toggle: visible → dismiss; hidden → summon.
                        let visible = handle
                            .get_webview_window("overlay")
                            .and_then(|w| w.is_visible().ok())
                            .unwrap_or(false);
                        if visible {
                            dismiss_overlay(&handle);
                        } else {
                            summon_overlay(&handle);
                        }
                    }
                })
                .map_err(|e| format!("global shortcut: {e}"))?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            diag,
            query_profile,
            get_orb_scene,
            get_findings,
            get_diagnosis,
            run_diagnosis,
            ask,
            hide_overlay,
            get_fix_preview,
            get_orb_fix_preview,
            resolve_evidence,
            apply_artifact,
            revert_artifact,
            start_voice,
            stop_voice,
            capture_screen,
            set_config,
            mute_pattern,
            get_radar_state,
            list_fleet,
            locate_agent,
            warp_to_agent
        ])
        .run(tauri::generate_context!())
        .expect("error while running WARDEN");
}
