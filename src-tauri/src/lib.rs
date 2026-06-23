pub mod brain;
pub mod commands;
pub mod config;
pub mod detectors;
pub mod featurizer;
pub mod forge;
pub mod harness_theme;
pub mod ingest;
pub mod ir;
pub mod redaction;
pub mod scaffold;
pub mod scheduler;
pub mod store;
pub mod util;

use commands::*;
use tauri::menu::MenuBuilder;
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
        let _ = app.emit("warden_hotkey", serde_json::json!({"hotkey":"cmd+shift+space"}));
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

            // 3) One-shot startup backfill so the HUD has data immediately. Runs off
            //    the UI thread; a failing adapter is isolated inside backfill_all and
            //    only logs — it never stalls the others or aborts startup.
            {
                let store = state.store.clone();
                tauri::async_runtime::spawn(async move {
                    let registry = ingest::AdapterRegistry::new(store.clone());
                    let summary = tokio::task::spawn_blocking(move || {
                        registry.backfill_all(&store)
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
            query_profile,
            get_findings,
            get_diagnosis,
            run_diagnosis,
            ask,
            hide_overlay,
            get_fix_preview,
            apply_artifact,
            revert_artifact,
            start_voice,
            stop_voice,
            capture_screen,
            set_config,
            mute_pattern,
            list_fleet,
            locate_agent,
            warp_to_agent
        ])
        .run(tauri::generate_context!())
        .expect("error while running WARDEN");
}
