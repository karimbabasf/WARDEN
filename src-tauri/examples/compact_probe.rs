//! Resolve every live radar agent through the compaction arm path, against the real
//! database and the real Claude session registry, and print what each one would do.
//!
//! This exists because the bug it was written to catch was invisible to the unit
//! suite. The tests addressed a session by its HARNESS id, while the radar hands the
//! command a STORE id, and no fixture ever crossed that gap, so a feature that was
//! dead for every live Claude session on the machine had a green suite behind it.
//! A probe that walks real ids is the only thing that would have said so.
//!
//! It never arms anything: this calls the resolver only, so nothing is persisted and
//! nothing is sent to any session.
//!
//!   cp ~/.warden/warden.db /tmp/warden_probe.db
//!   WARDEN_DB_PATH=/tmp/warden_probe.db cargo run --example compact_probe

use warden_lib::{compact, radar, store::Store, util};

fn main() -> anyhow::Result<()> {
    let db = util::default_db_path();
    println!("db: {}", db.display());
    let store = Store::open(&db)?;
    let sessions_dir = util::default_claude_sessions_dir();
    let codex_dir = util::default_codex_sessions();

    let state = radar::recompute_radar_state(&store, &sessions_dir);
    println!("{} agents on the board\n", state.agents.len());

    let mut armable = 0usize;
    for a in &state.agents {
        let label = a.nickname.clone().unwrap_or_else(|| a.label.clone());
        match compact::resolve_for_probe(&store, &sessions_dir, &codex_dir, &a.id) {
            Ok(rec) => {
                armable += 1;
                println!(
                    "  OK    {:<28} {:<12} {:<10} mode={} idle={}\n        {}",
                    truncate(&label, 28),
                    a.harness,
                    a.status,
                    rec.mode.as_str(),
                    rec.idle_source.as_str(),
                    rec.mode_reason,
                );
            }
            Err(e) => println!(
                "  FAIL  {:<28} {:<12} {:<10} {}",
                truncate(&label, 28),
                a.harness,
                a.status,
                format!("{e:#}")
            ),
        }
    }
    println!("\n{armable}/{} agents can be armed", state.agents.len());
    Ok(())
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    s.chars().take(n - 1).collect::<String>() + "."
}
