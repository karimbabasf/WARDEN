//! Diagnostic: run the Grok adapter against the REAL `~/.grok/sessions` tree.
//!
//! Read-only, in-memory store, nothing persisted. The unit tests prove the parser
//! against records typed by hand; this proves the layout assumptions (two levels
//! deep, the cwd in the directory name) against the sessions actually on disk.
//!
//!   cargo run --manifest-path src-tauri/Cargo.toml --example grok_probe

use warden_lib::ingest::{grok::GrokAdapter, Adapter};
use warden_lib::store::Store;

fn main() {
    let store = Store::memory().expect("in-memory store");
    let adapter = GrokAdapter::new(store);
    let files = adapter.detect().expect("detect");
    println!("grok session files: {}", files.len());
    if files.is_empty() {
        println!("  (none under ~/.grok/sessions)");
        return;
    }

    let batches = adapter.backfill().expect("backfill");
    println!("parsed sessions:    {}\n", batches.len());
    println!(
        "{:<40} {:<12} {:<7} {:<7} {}",
        "SESSION", "MODEL", "TURNS", "EVENTS", "CWD"
    );
    for b in &batches {
        println!(
            "{:<40} {:<12} {:<7} {:<7} {}",
            b.session.external_id,
            b.session.model_ids.first().map(String::as_str).unwrap_or("-"),
            b.turns.len(),
            b.events.len(),
            b.session
                .project
                .as_ref()
                .map(|p| p.cwd.display().to_string())
                .unwrap_or_else(|| "(no cwd)".into()),
        );
    }

    // Event mix, so a harness that turns out to be all noise is obvious.
    let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
    let mut notices: std::collections::BTreeMap<String, usize> = Default::default();
    for b in &batches {
        for e in &b.events {
            *kinds.entry(e.event.kind_name()).or_default() += 1;
            if let warden_lib::ir::Event::SystemNotice { subtype, .. } = &e.event {
                *notices.entry(subtype.clone()).or_default() += 1;
            }
        }
    }
    println!("\nevent kinds:");
    for (k, n) in kinds {
        println!("  {k:<16} {n}");
    }
    println!("\nsystem notices:");
    for (k, n) in notices {
        println!("  {k:<28} {n}");
    }
}
