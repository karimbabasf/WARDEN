//! Throwaway diagnostic: run the REAL radar computation against live local data and
//! print the forest it produces. Non-destructive — point WARDEN_DB_PATH at a copy of
//! the db (the relink step writes), while liveness reads the real ~/.claude & ~/.codex.
//!
//!   cp ~/.warden/warden.db /tmp/warden_probe.db
//!   WARDEN_DB_PATH=/tmp/warden_probe.db cargo run --example radar_probe

use warden_lib::radar::recompute_radar_state;
use warden_lib::store::Store;
use warden_lib::util::{default_claude_sessions_dir, default_db_path};

fn main() {
    let dbp = default_db_path();
    let reg = default_claude_sessions_dir();
    eprintln!("db       = {dbp:?}");
    eprintln!("registry = {reg:?}");

    let store = Store::open(&dbp).expect("open store");
    let total_sessions = store.sessions().map(|s| s.len()).unwrap_or(0);
    eprintln!("store sessions (all) = {total_sessions}");

    let state = recompute_radar_state(&store, &reg);

    // Dump the EXACT bytes the `get_radar_state` IPC command returns, so a throwaway
    // browser harness can render the real forest through the real normalize seam.
    let json = serde_json::to_string_pretty(&state).expect("serialize radar state");
    let out = "/Users/karimbaba/WARDEN/src/viz/preview/realRadar.json";
    std::fs::write(out, &json).expect("write realRadar.json");
    eprintln!("wrote {} ({} bytes)", out, json.len());

    println!("generated_at = {}", state.generated_at);
    println!("RADAR AGENTS (globes) = {}", state.agents.len());
    for a in &state.agents {
        let indent = "  ".repeat(a.depth as usize + 1);
        println!(
            "{indent}[{}] {} | status={} depth={} fill={:.1} ctx={}/{} kids={} model={:?} role={:?} parent={:?}",
            a.harness,
            a.label,
            a.status,
            a.depth,
            a.fill_pct,
            a.context_tokens,
            a.max_tokens,
            a.child_count,
            a.model,
            a.role,
            a.parent_id,
        );
    }
}
