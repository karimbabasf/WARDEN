//! Throwaway diagnostic: what does "take me there" see for every live agent, and
//! how long does one radar recompute actually take? Non-destructive: point
//! WARDEN_DB_PATH at a copy of the db (the relink step writes), while liveness
//! reads the real ~/.claude and ~/.codex.
//!
//!   cp ~/.warden/warden.db /tmp/warden_probe.db
//!   WARDEN_DB_PATH=/tmp/warden_probe.db cargo run --example terminal_probe

use std::time::Instant;
use warden_lib::radar::liveness::{pid_alive, read_claude_registry};
use warden_lib::radar::recompute_radar_state;
use warden_lib::store::Store;
use warden_lib::terminal::locate;
use warden_lib::util::{default_claude_sessions_dir, default_db_path};

fn main() {
    let dbp = default_db_path();
    let reg_dir = default_claude_sessions_dir();
    let store = Store::open(&dbp).expect("open store");

    // Timing: this is what every radar emit costs, so it bounds how fast a globe
    // can bloom or implode.
    let t0 = Instant::now();
    let state = recompute_radar_state(&store, &reg_dir);
    let recompute_ms = t0.elapsed().as_millis();
    println!("recompute_radar_state: {recompute_ms} ms for {} agents", state.agents.len());

    let registry = read_claude_registry(&reg_dir);
    println!("registry entries: {}", registry.len());
    println!();

    for a in &state.agents {
        let located = locate(
            &state.agents,
            |id| store.session_by_id(id).ok().flatten(),
            &registry,
            &a.id,
            pid_alive,
            warden_lib::platform::controlling_tty,
            warden_lib::platform::terminal_host_for_pid,
        );
        let t = &located.target;
        let verdict = if t.reachable {
            format!("REACHABLE via {}", t.app.clone().unwrap_or_default())
        } else {
            format!("blocked: {}", t.reason.clone().unwrap_or_default())
        };
        println!(
            "{:<34} {:<10} depth={} {}",
            a.label.chars().take(34).collect::<String>(),
            a.status,
            a.depth,
            verdict
        );
    }
}
