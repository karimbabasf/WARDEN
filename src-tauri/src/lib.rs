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

/// Holds the RADAR liveness watchers AND the single recompute worker so they
/// outlive `setup()`. A distinct type from `scheduler::WatcherGuard` because Tauri's
/// managed state is keyed by type — the ingest watchers and the radar watchers must
/// each get their own slot. The worker `JoinHandle` is parked here so the coalescing
/// recompute task lives for the app's lifetime (Fix #1).
struct RadarWatcherGuard {
    #[allow(dead_code)]
    watchers: std::sync::Mutex<Vec<notify::RecommendedWatcher>>,
    #[allow(dead_code)]
    worker: tauri::async_runtime::JoinHandle<()>,
    #[allow(dead_code)]
    tick: tauri::async_runtime::JoinHandle<()>,
    /// The radar dirty signal, parked here so a recompute can be kicked from outside
    /// the watcher (e.g. once startup backfill has populated the store).
    #[allow(dead_code)]
    radar_signal: scheduler::RadarDirtySignal,
}
use tauri::tray::TrayIconBuilder;
use tauri::{ActivationPolicy, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Reveal the persistent overlay window: show, focus, and signal the frontend to
/// animate in. Native macOS chrome owns drag, zoom, and resize, so we show the
/// window at its own size rather than force-maximizing.
/// Idempotent — safe to call when already visible.
fn summon_overlay(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.show();
        let _ = w.set_focus();
        let _ = app.emit(
            "warden_hotkey",
            serde_json::json!({"hotkey":"cmd+option+control+m"}),
        );
    }
}

/// Hide the overlay window. The daemon keeps running and the window is
/// re-summonable via the ⌘⌥⌃M hotkey or the tray menu. Drives the hotkey
/// toggle.
fn dismiss_overlay(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("overlay") {
        let _ = w.hide();
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
            // 1) Persistent app: show a Dock icon so Minimize has a home and the
            //    window behaves like a regular macOS app. The overlay is still
            //    created hidden (tauri.conf.json visible:false) and summoned via
            //    the hotkey/tray; the daemon stays alive when the window is hidden.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(ActivationPolicy::Regular);

            let state = AppState::init().map_err(|e| format!("state init: {e}"))?;

            // 2) RADAR liveness watchers: the Claude `~/.claude/sessions` registry
            //     (bloom/implode) + the Codex live/archived roots (archive-move =
            //     done). On any change the whole forest is recomputed and pushed as
            //     `radar_state`. Best-effort, same isolation as the ingest watchers.
            let radar_roots = vec![
                util::default_codex_sessions(),
                util::default_codex_archived_sessions(),
                util::default_claude_projects(),
            ];
            // Kept outside the match so the startup backfill (step 3) can kick a second
            // recompute once the store is populated (cold-DB-with-live-agents case).
            let mut radar_signal: Option<scheduler::RadarDirtySignal> = None;
            match scheduler::spawn_radar_watcher(
                state.store.clone(),
                util::default_claude_sessions_dir(),
                radar_roots,
                app.handle().clone(),
                state.radar_state.clone(),
            ) {
                Ok((watchers, worker, tick, signal)) => {
                    radar_signal = Some(signal.clone());
                    app.manage(RadarWatcherGuard {
                        watchers: std::sync::Mutex::new(watchers),
                        worker,
                        tick,
                        radar_signal: signal,
                    });
                }
                Err(e) => {
                    tracing::warn!(error=%format!("{e:#}"), "radar watchers failed to start")
                }
            }

            // 2b) Live FSEvents watchers (one per adapter root). Best-effort: a watch
            //     failure logs and never aborts startup — backfill + on-ask ingest
            //     still work. A successful live ingest also kicks RADAR's dirty signal,
            //     so the overlay updates from the ingested tail without waiting for a
            //     second watcher event or the heartbeat tick.
            let watch_registry =
                std::sync::Arc::new(ingest::AdapterRegistry::new(state.store.clone()));
            match scheduler::spawn_watchers(
                watch_registry,
                state.store.clone(),
                app.handle().clone(),
                radar_signal.clone(),
            ) {
                Ok(watchers) => {
                    app.manage(scheduler::WatcherGuard::new(watchers));
                }
                Err(e) => tracing::warn!(error=%format!("{e:#}"), "live watchers failed to start"),
            }

            // 3) One-shot startup backfill so the HUD has data immediately. Runs off
            //    the UI thread; a failing adapter is isolated inside backfill_all and
            //    only logs — it never stalls the others or aborts startup.
            {
                let store = state.store.clone();
                let app_handle = app.handle().clone();
                let radar_signal = radar_signal.clone();
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
                    // Re-derive the live forest now that the store is fully populated.
                    // The watcher already kicked once at spawn (over the persistent DB +
                    // live registry); this second kick reflects sessions/events that only
                    // existed after backfill — so already-running agents are tracked at
                    // launch instead of waiting for the next unrelated FS write.
                    if let Some(signal) = &radar_signal {
                        tracing::info!("startup backfill complete; kicking radar recompute");
                        signal.mark_dirty();
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

            // 5) The overlay is a persistent app window, created hidden
            //    (tauri.conf.json visible:false) and revealed on summon. It is no
            //    longer click-through and no longer dismisses on blur/focus-loss —
            //    it stays put until the user minimizes/hides it or toggles the
            //    hotkey. Dismissal is now explicit (tray, hotkey, or the
            //    minimize_window / hide_window commands).

            // 6) Global hotkey ⌘⌥⌃M, replacing the old ⌘⇧Space. Guard with
            //    is_registered so a hot-reload / re-setup does not double-register.
            let handle = app.handle().clone();
            let shortcut = Shortcut::new(
                Some(Modifiers::SUPER | Modifiers::ALT | Modifiers::CONTROL),
                Code::KeyM,
            );
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

            // Show the window on launch at its default size; native chrome lets the
            // user zoom or resize from there. The overlay then stays put — it pauses
            // only on minimize and never hides on blur. Remove this single call to
            // revert to hotkey-only summon.
            summon_overlay(&app.handle());
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
            minimize_window,
            hide_window,
            get_fix_preview,
            get_orb_fix_preview,
            resolve_evidence,
            stage_artifact,
            apply_artifact,
            revert_artifact,
            get_artifact,
            list_artifacts,
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
        .on_window_event(|window, event| {
            // The red traffic-light button (and ⌘W) asks the window to CLOSE. WARDEN
            // is a daemon, so a close must not quit it: veto the request and hide the
            // window instead. Re-summon via ⌘⌥⌃M, the tray, or the Dock icon (the
            // Reopen handler below). This is the single seam where the OS's "close"
            // verb is translated into WARDEN's "hide" verb.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "overlay" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building WARDEN")
        .run(|app, event| {
            // macOS: clicking the Dock icon while the overlay is hidden re-summons
            // it — the standard "reopen" gesture for a window that closes-to-hide.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                summon_overlay(app);
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (&app, &event);
            }
        });
}
