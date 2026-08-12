//! Throwaway diagnostic: watch the REAL radar verdict for live agents, once a second.
//!
//! Read-only toward the harness. It backfills into a scratch DB (never `~/.warden`), then
//! runs the real `recompute_radar_state` in a loop and prints every agent whose status
//! changed, with the action that decided it. Point it at a live machine and drive agents
//! in another window: a correct rule shows `working` for as long as the agent is
//! producing, and flips to `idle` when it actually stops.
//!
//!   cargo run --example live_watch -- [seconds]

use chrono::Utc;
use std::collections::HashMap;
use warden_lib::ingest::AdapterRegistry;
use warden_lib::radar::recompute_radar_state;
use warden_lib::store::Store;
use warden_lib::util::default_claude_sessions_dir;

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    // A fresh file per run: a previous run killed mid-write leaves a partial db behind,
    // and sqlite refuses to open it. Clear the sidecars too, not just the main file.
    let db = std::env::temp_dir().join(format!("warden_live_watch_{}.db", std::process::id()));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
    let store = Store::open(&db).expect("open scratch store");
    let reg_dir = default_claude_sessions_dir();

    eprintln!("scratch db = {}", db.display());
    eprintln!("backfilling live transcripts (fresh, so events carry the stop reason)");
    let summary = AdapterRegistry::new(store.clone()).backfill_all(&store);
    for (h, sessions, events) in &summary.by_harness {
        eprintln!("  {:?}: {sessions} sessions, {events} events", h);
    }
    for e in &summary.errors {
        eprintln!("  error: {e}");
    }

    let mut last: HashMap<String, String> = HashMap::new();
    let started = std::time::Instant::now();
    let mut tick = 0u32;

    while started.elapsed().as_secs() < secs {
        // Pick up whatever the live agents have written since the previous pass.
        AdapterRegistry::new(store.clone()).backfill_all(&store);
        let state = recompute_radar_state(&store, &reg_dir);

        let (mut working, mut idle, mut other) = (0, 0, 0);
        for a in &state.agents {
            match a.status.as_str() {
                "working" => working += 1,
                "idle" => idle += 1,
                _ => other += 1,
            }
        }
        let now = Utc::now().format("%H:%M:%S");
        if tick == 0 {
            println!(
                "\n{now}  {} agents: {working} working, {idle} idle, {other} other",
                state.agents.len()
            );
            for a in &state.agents {
                let kind = if a.depth == 0 { "root" } else { "SUB " };
                println!(
                    "  {kind} {:<8} d{} {:<34} {}",
                    a.status,
                    a.depth,
                    truncate(&a.label, 34),
                    a.current_action.as_ref().map(|c| c.label.as_str()).unwrap_or("")
                );
            }
        }
        for a in &state.agents {
            let prev = last.get(&a.id);
            if prev.is_some_and(|p| p != &a.status) {
                let kind = if a.depth == 0 { "root" } else { "SUB " };
                println!(
                    "{now}  {kind} {:<34} {:>8} -> {:<8}  {}",
                    truncate(&a.label, 34),
                    prev.map(String::as_str).unwrap_or("?"),
                    a.status,
                    a.current_action.as_ref().map(|c| c.label.as_str()).unwrap_or("")
                );
            }
            last.insert(a.id.clone(), a.status.clone());
        }
        tick += 1;
        std::thread::sleep(std::time::Duration::from_millis(1000));
    }

    println!("\n=== final ===");
    let state = recompute_radar_state(&store, &reg_dir);
    for a in &state.agents {
        let kind = if a.depth == 0 { "root" } else { "SUB " };
        println!(
            "  {kind} {:<8} d{} {:<34} {}",
            a.status,
            a.depth,
            truncate(&a.label, 34),
            a.current_action.as_ref().map(|c| c.label.as_str()).unwrap_or("")
        );
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n.saturating_sub(1)).collect::<String>() + ">"
    }
}
