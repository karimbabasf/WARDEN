//! Diagnostic: run the REAL process sweep and print every agent it finds.
//!
//! Read-only, no database, no arguments. This exists because the unit tests can
//! only prove the parser is right about strings someone typed; only a live sweep
//! proves the signature table matches the binaries actually installed here.
//!
//!   cargo run --manifest-path src-tauri/Cargo.toml --example procs_probe

use warden_lib::platform::list_agent_processes;

fn main() {
    let Some(procs) = list_agent_processes() else {
        println!("process sweep FAILED (ps did not run). RADAR would defer to file rules.");
        return;
    };
    println!("agent processes: {}", procs.len());
    if procs.is_empty() {
        println!("  (none: no harness is running, or the signature table missed one)");
        return;
    }
    // The directory is what a harness with no pid registry is matched on, so a
    // blank column here is the difference between "closes on kill" and "defers".
    let cwds = warden_lib::platform::process_cwds(&procs.iter().map(|p| p.pid).collect::<Vec<_>>());
    println!(
        "{:<8} {:<8} {:<12} {:<14} {:<26} {}",
        "PID", "PPID", "HARNESS", "ARGV0", "STARTED", "CWD"
    );
    for p in &procs {
        println!(
            "{:<8} {:<8} {:<12} {:<14} {:<26} {}",
            p.pid,
            p.ppid,
            p.harness.as_str(),
            p.argv0,
            p.started_at,
            cwds.get(&p.pid).map(String::as_str).unwrap_or("(unreadable)")
        );
    }

    // Per-harness tally, so a missing harness is obvious at a glance.
    let mut by: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for p in &procs {
        *by.entry(p.harness.as_str()).or_default() += 1;
    }
    println!("\nby harness:");
    for (h, n) in by {
        println!("  {h:<12} {n}");
    }
}
