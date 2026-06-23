pub mod brain;
pub mod commands;
pub mod detectors;
pub mod featurizer;
pub mod ingest;
pub mod ir;
pub mod redaction;
pub mod scaffold;
pub mod store;
pub mod util;

use commands::*;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            let state = AppState::init().map_err(|e| format!("state init: {e}"))?;
            app.manage(state);
            let handle = app.handle().clone();
            let shortcut = Shortcut::new(Some(Modifiers::ALT), Code::Space);
            app.global_shortcut()
                .on_shortcut(shortcut, move |_app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let _ = handle.emit(
                            "warden_hotkey",
                            serde_json::json!({"hotkey":"option+space"}),
                        );
                        if let Some(w) = handle.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .map_err(|e| format!("global shortcut: {e}"))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            query_profile,
            get_findings,
            get_diagnosis,
            run_diagnosis,
            ask,
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
