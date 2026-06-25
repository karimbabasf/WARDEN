//! Throwaway diagnostic: run the REAL radar computation against live local data and
//! print the forest it produces. Non-destructive — point WARDEN_DB_PATH at a copy of
//! the db (the relink step writes), while liveness reads the real ~/.claude & ~/.codex.
//!
//!   cp ~/.warden/warden.db /tmp/warden_probe.db
//!   WARDEN_DB_PATH=/tmp/warden_probe.db cargo run --example radar_probe

use warden_lib::radar::liveness::{pid_alive, read_claude_registry};
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

    // ── Raw registry ground truth (what liveness actually reads) ────────────────
    let registry = read_claude_registry(&reg);
    println!("\n=== CLAUDE REGISTRY ({} entries) ===", registry.len());
    for (pid, v) in &registry {
        let alive = pid_alive(*pid);
        let sid = v.get("sessionId").and_then(|x| x.as_str()).unwrap_or("?");
        let cwd = v.get("cwd").and_then(|x| x.as_str()).unwrap_or("?");
        let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("<none>");
        let ver = v.get("version").and_then(|x| x.as_str()).unwrap_or("?");
        println!("  pid={pid} alive={alive} reg_status={status} ver={ver} sid={sid} cwd={cwd}");
    }

    let state = recompute_radar_state(&store, &reg);

    // Dump the EXACT bytes the `get_radar_state` IPC command returns, so a throwaway
    // browser harness can render the real forest through the real normalize seam.
    let json = serde_json::to_string_pretty(&state).expect("serialize radar state");
    let out = "/Users/karimbaba/WARDEN/src/viz/preview/realRadar.json";
    std::fs::write(out, &json).expect("write realRadar.json");
    eprintln!("wrote {} ({} bytes)", out, json.len());

    println!("\ngenerated_at = {}", state.generated_at);
    println!("RADAR AGENTS (globes) = {}", state.agents.len());
    let (mut working, mut idle, mut closed) = (0, 0, 0);
    for a in &state.agents {
        match a.status.as_str() {
            "working" => working += 1,
            "idle" => idle += 1,
            "closed" => closed += 1,
            _ => {}
        }
        let indent = "  ".repeat(a.depth as usize + 1);
        println!(
            "{indent}[{}] {} | status={} cwd={:?} depth={} fill={:.1} ctx={}/{} kids={} model={:?} role={:?} parent={:?}",
            a.harness,
            a.label,
            a.status,
            a.cwd,
            a.depth,
            a.fill_pct,
            a.context_tokens,
            a.max_tokens,
            a.child_count,
            a.model,
            a.role,
            a.parent_id,
        );
        // The "what is it doing" signal — files touched / commands run / messages.
        if a.recent_activity.is_empty() {
            println!("{indent}    activity: <none>");
        } else {
            for act in a.recent_activity.iter().take(6) {
                println!("{indent}    · [{}] {}", act.kind, act.label);
            }
        }
    }
    println!("\nSUMMARY: working={working} idle={idle} closed={closed}");
}
