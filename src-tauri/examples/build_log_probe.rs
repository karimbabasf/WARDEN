//! Brickyard proof: fold one session's stored events into its build log and print the
//! counts, so they can be checked against the raw transcript. Point WARDEN_DB_PATH at a
//! copy of the db (opening the store migrates it).
//!
//!   cp -c ~/.warden/warden.db /tmp/warden_probe.db
//!   WARDEN_DB_PATH=/tmp/warden_probe.db cargo run --example build_log_probe -- <session-id>

use std::collections::BTreeMap;
use warden_lib::radar::{build_log, BuildAction};
use warden_lib::store::Store;
use warden_lib::util::default_db_path;

fn main() {
    let sid = std::env::args().nth(1).expect("usage: build_log_probe <session-id>");
    let store = Store::open(default_db_path()).expect("open store");
    let events = store.build_events(&sid).expect("read events");
    let log = build_log(&events);
    let mut by_action: BTreeMap<String, usize> = BTreeMap::new();
    for s in &log {
        *by_action.entry(format!("{:?}", s.action)).or_default() += 1;
    }
    let placed = log.iter().filter(|s| s.action == BuildAction::Place).count();
    let pulled = log.iter().filter(|s| s.action == BuildAction::Pull).count();
    println!("session {sid}: {} log entries", log.len());
    for (k, v) in &by_action {
        println!("  {k:<7} {v}");
    }
    println!("bricks standing = {placed} placed - {pulled} pulled = {}", placed - pulled);
    // `--json <file>` also writes the log, for replaying a real session in the lab.
    let args: Vec<String> = std::env::args().collect();
    if let Some(i) = args.iter().position(|a| a == "--json") {
        let out = args.get(i + 1).expect("--json needs a file");
        std::fs::write(out, serde_json::to_string(&log).expect("log serializes")).expect("write json");
    }
}
