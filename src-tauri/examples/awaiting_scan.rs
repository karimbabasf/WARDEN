//! Throwaway diagnostic: score the AWAITING rule against real local transcripts.
//!
//! Read-only. It replays local Claude transcripts through the REAL parser and the REAL
//! [`finished_turn_asks_question`] / [`is_blocking_tool`], so what it reports is what the
//! radar would do, not a re-implementation of it.
//!
//! Three things worth knowing before trusting a red globe:
//!
//! 1. **How often does the prose rule fire?** It only ever upgrades a QUIET verdict, so
//!    the denominator is completed turns. A few percent means "agents sometimes end on a
//!    question", which is true. Twenty percent would mean the rule is reading ordinary
//!    prose as a prompt, and a red globe you cannot trust is worse than no red globe.
//! 2. **Does it catch the real ones?** Every `AskUserQuestion` in the corpus is a turn
//!    where the operator demonstrably had to answer. Any that the structured rule would
//!    miss are the recall gap.
//! 3. **What does it actually look like?** A handful of the matched texts, printed, so
//!    the rate can be judged rather than just counted.
//!
//!   cargo run --example awaiting_scan

use warden_lib::ingest::claude_code::ClaudeCodeAdapter;
use warden_lib::ingest::Adapter;
use warden_lib::ir::{Event, EventRecord, Turn};
use warden_lib::radar::awaiting::{finished_turn_asks_question, is_blocking_tool};
use warden_lib::store::Store;
use warden_lib::util::default_claude_projects;

fn main() {
    let root = default_claude_projects();
    let store = Store::memory().expect("memory store");
    let adapter = ClaudeCodeAdapter::with_root(root.clone(), store);
    let files = adapter.detect().unwrap_or_default();
    println!("scanning {} transcripts under {}", files.len(), root.display());

    let mut completed_turns = 0u32;
    let mut asked = 0u32;
    let mut blocking_calls = 0u32;
    let mut samples: Vec<String> = Vec::new();
    let mut sample_misses: Vec<String> = Vec::new();

    for p in &files {
        let Ok(bytes) = std::fs::read(p) else { continue };
        let Ok(batches) = adapter.parse_range(p, &bytes, 0, 0) else {
            continue;
        };
        let mut all: Vec<(Turn, EventRecord)> = Vec::new();
        for batch in &batches {
            let Some(t0) = batch.turns.first().cloned() else {
                continue;
            };
            all.extend(batch.events.iter().cloned().map(|e| (t0.clone(), e)));
        }
        all.sort_by(|(_, a), (_, b)| a.ts.cmp(&b.ts).then(a.raw_ref.offset.cmp(&b.raw_ref.offset)));

        for i in 0..all.len() {
            match &all[i].1.event {
                // A completed assistant turn is the decision point for the prose rule.
                Event::AssistantText {
                    text,
                    turn_complete,
                } => {
                    if *turn_complete == Some(false) {
                        continue;
                    }
                    completed_turns += 1;
                    if finished_turn_asks_question(&all[..=i]) {
                        asked += 1;
                        if samples.len() < 12 {
                            samples.push(last_line(text));
                        }
                    }
                }
                // Ground truth for recall: the operator provably had to answer this one.
                Event::ToolCall { tool, .. } if is_blocking_tool(tool) => {
                    blocking_calls += 1;
                }
                _ => {}
            }
        }

        // Recall check: for each blocking call, would the rule have fired at the moment
        // it was still unresolved? That is what the radar sees while the dialog is open.
        for i in 0..all.len() {
            let Event::ToolCall { tool, .. } = &all[i].1.event else {
                continue;
            };
            if !is_blocking_tool(tool) {
                continue;
            }
            // The prefix up to and including the call is exactly the transcript state
            // while the question is on screen.
            let now = all[i].1.ts + chrono::Duration::seconds(1);
            let caught = warden_lib::radar::awaiting::blocking_call_in_flight(&all[..=i], now);
            if caught.is_none() && sample_misses.len() < 5 {
                sample_misses.push(format!("{tool} at {}", all[i].1.ts));
            }
        }
    }

    let pct = |n: u32, d: u32| if d == 0 { 0.0 } else { (n as f64 / d as f64) * 100.0 };
    println!("\n=== prose rule (only ever upgrades a QUIET verdict) ===");
    println!("  completed assistant turns : {completed_turns}");
    println!(
        "  read as a question        : {asked}  ({:.1}% of completed turns)",
        pct(asked, completed_turns)
    );
    println!("\n=== structured rule ===");
    println!("  AskUserQuestion / ExitPlanMode calls in the corpus : {blocking_calls}");
    println!(
        "  ones the in-flight rule would MISS while open      : {}",
        sample_misses.len()
    );
    for m in &sample_misses {
        println!("      miss: {m}");
    }
    println!("\n=== a sample of what the prose rule matched (judge these) ===");
    for s in &samples {
        println!("  · {s}");
    }
}

/// The trailing line of a message, trimmed for printing.
fn last_line(text: &str) -> String {
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let clipped: String = line.chars().take(110).collect();
    clipped
}
