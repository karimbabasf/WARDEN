//! Throwaway diagnostic: build a REAL DOSSIER profile against live local data and
//! print it. Non-destructive — point WARDEN_DB_PATH at a COPY of the db (build_profile
//! writes the dossier_* cache tables). Forces detector-only (no brain) for a fast,
//! deterministic, network-free proof of the whole backend pipeline on real data.
//!
//!   cp ~/.warden/warden.db /tmp/warden_dossier.db
//!   WARDEN_BRAIN_API_KEY= OPENAI_API_KEY= WARDEN_DB_PATH=/tmp/warden_dossier.db \
//!     cargo run --example dossier_smoke

use chrono::Utc;
use warden_lib::brain::Brain;
use warden_lib::dossier::build::build_profile;
use warden_lib::dossier::scope::Window;
use warden_lib::store::Store;
use warden_lib::util::default_db_path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let dbp = default_db_path();
    eprintln!("db = {dbp:?}");
    let store = Store::open(&dbp)?;
    let brain = Brain::new(store.clone());
    eprintln!("brain available = {}", brain.available());

    let profile = build_profile(&store, &brain, Window::AllTime, Utc::now(), None).await?;

    println!("\n=== DOSSIER PROFILE (all-time) ===");
    println!(
        "detector_only = {}   sessions = {}   rubric = {}",
        profile.detector_only, profile.session_count, profile.rubric_version
    );
    println!("\nEFFICIENCY headline = {:.3}  ({} / 100)", profile.efficiency.headline, (profile.efficiency.headline * 100.0).round() as i64);
    for f in &profile.efficiency.families {
        let bar = "#".repeat((f.sub_score * 24.0).round() as usize);
        println!("  {:26} {:.3}  w={:.2}  {}", f.key, f.sub_score, f.weight, bar);
    }

    println!("\nDIMENSIONS ({}):", profile.dimensions.len());
    for d in &profile.dimensions {
        println!("  [{}] {} — {} claims", d.key, d.title, d.claims.len());
        if !d.narrative.is_empty() {
            let n: String = d.narrative.chars().take(180).collect();
            println!("       {n}");
        }
    }

    println!("\nARCHETYPES ({}):", profile.archetypes.len());
    for a in &profile.archetypes {
        println!("  {} — {} sessions — {}", a.archetype, a.session_count, a.note);
    }

    println!("\nTOP LEAKS:");
    for l in profile.ranked_leaks.iter().take(5) {
        println!("  #{} {} (~{} tok / ~{} min)", l.rank, l.title, l.est_cost_tokens, l.est_cost_minutes);
    }

    println!("\nTRAJECTORY:");
    for t in &profile.trajectory {
        println!("  {} — {:?} (conf {:.2}, {} points)", t.trait_key, t.direction, t.confidence, t.points.len());
    }
    Ok(())
}
