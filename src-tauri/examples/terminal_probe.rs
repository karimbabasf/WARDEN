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
    // can bloom or implode. The two store reads are broken out because they are
    // the parts that grow with the store rather than with the live forest.
    let t = Instant::now();
    let all = store.sessions().unwrap_or_default();
    println!("store.sessions():      {} ms for {} rows", t.elapsed().as_millis(), all.len());

    let t = Instant::now();
    let links = store.parent_links().unwrap_or_default();
    println!("store.parent_links():  {} ms for {} rows", t.elapsed().as_millis(), links.len());

    // The urgent path (a session starting or ending) runs refresh_live_context
    // BEFORE the recompute, so the felt latency is the sum of the two, not the
    // recompute alone.
    warden_lib::radar::refresh_live_context(&store, &reg_dir);
    let mut refreshes = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        warden_lib::radar::refresh_live_context(&store, &reg_dir);
        refreshes.push(t.elapsed().as_millis());
    }
    refreshes.sort();
    println!(
        "refresh_live_context:  median {} ms (runs {:?})",
        refreshes[refreshes.len() / 2],
        refreshes
    );

    // refresh_live_context runs this whenever ANY new transcript bytes land.
    let mut links = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        let _ = warden_lib::ingest::claude_code::link_claude_subagents_in_store(&store);
        links.push(t.elapsed().as_millis());
    }
    links.sort();
    println!("link_claude_subagents:  median {} ms (runs {:?})", links[1], links);

    // Five warm runs, not one: the first read of a cold page cache is not the
    // number the operator feels on the tenth agent start of a session.
    let mut state = recompute_radar_state(&store, &reg_dir);
    let mut runs = Vec::new();
    for _ in 0..5 {
        let t0 = Instant::now();
        state = recompute_radar_state(&store, &reg_dir);
        runs.push(t0.elapsed().as_millis());
    }
    runs.sort();
    println!(
        "recompute_radar_state: median {} ms (runs {:?}) for {} agents",
        runs[runs.len() / 2],
        runs,
        state.agents.len()
    );

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
