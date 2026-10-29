//! Throwaway diagnostic: where does the radar's steady-state CPU actually go?
//!
//! Prints the cost of each stage the recompute worker drives, so a regression in the
//! ingest fast path or the process-sweep cache is one command away instead of a guess.
//! Non-destructive to the real store — point `WARDEN_DB_PATH` at a copy, the refresh
//! stage WRITES.
//!
//!   cp ~/.warden/warden.db /tmp/warden_probe.db
//!   WARDEN_DB_PATH=/tmp/warden_probe.db cargo run --release --example perf_probe

use std::time::Instant;
use warden_lib::radar::{recompute_radar_state, refresh_live_context};
use warden_lib::store::Store;
use warden_lib::util::{default_claude_sessions_dir, default_db_path};

/// Median of `n` runs, so one cold page-cache miss does not read as the steady state.
fn bench<T>(label: &str, n: usize, mut body: impl FnMut() -> T) -> T {
    let mut ms = Vec::with_capacity(n);
    let mut last = None;
    for _ in 0..n {
        let t = Instant::now();
        last = Some(body());
        ms.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(f64::total_cmp);
    println!("{label:<34}{:>9.1}ms  (median of {n})", ms[n / 2]);
    last.expect("n >= 1")
}

fn main() {
    let store = Store::open(default_db_path()).expect("open store");
    let reg = default_claude_sessions_dir();

    let sessions = bench("store.sessions()", 5, || store.sessions().unwrap_or_default());
    println!("  {} session rows", sessions.len());
    bench("store.parent_links()", 5, || {
        store.parent_links().unwrap_or_default()
    });
    bench("agent_task_call_parents()", 5, || {
        store.agent_task_call_parents().unwrap_or_default()
    });
    bench("link_claude_subagents_in_store()", 3, || {
        warden_lib::ingest::claude_code::link_claude_subagents_in_store(&store).unwrap_or(0)
    });
    bench("platform::process_index()", 3, || {
        warden_lib::platform::process_index()
    });

    // FIRST refresh reads every live transcript in full; every one after it should read
    // nothing at all until a file actually moves. The gap between the two is the whole
    // point of the size/mtime mark on `watermarks`.
    let n = bench("refresh_live_context [cold]", 1, || {
        refresh_live_context(&store, &reg)
    });
    println!("  {n} events ingested");
    let n = bench("refresh_live_context [steady]", 3, || {
        refresh_live_context(&store, &reg)
    });
    println!("  {n} events ingested");

    let state = bench("recompute_radar_state()", 5, || {
        recompute_radar_state(&store, &reg)
    });
    println!("  {} agents in the forest", state.agents.len());
}
