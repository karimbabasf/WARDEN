use anyhow::Result;
use warden_lib::{brain::Brain, featurizer, ingest::claude_code, ir::RunScope, store::Store, util::default_db_path};

#[tokio::main]
async fn main() -> Result<()> {
    let args:Vec<String>=std::env::args().collect();
    let store=Store::open(default_db_path())?;
    match args.get(1).map(|s|s.as_str()).unwrap_or("diagnose") {
        "ingest" => { let (s,e)=claude_code::ingest_all(&store,None,None)?; let p=featurizer::compute_all(&store)?; println!("ingested_sessions={s} ingested_events={e} total_sessions={} total_events={}",p.session_count,p.event_count); },
        "detectors" => { let p=featurizer::compute_all(&store)?; let fs=warden_lib::detectors::nominate(&store,&p)?; println!("{}",serde_json::to_string_pretty(&fs)?); },
        "diagnose" => { let (s,e)=claude_code::ingest_all(&store,None,None)?; eprintln!("ingest delta: sessions={s} events={e}"); let d=Brain::new(store).run_pipeline(RunScope{harness:Some("claude_code".into()),query:Some("what's wrong with how I use my agents?".into()),force:Some(false)}).await?; println!("{}",serde_json::to_string_pretty(&d)?); },
        other => anyhow::bail!("unknown command {other}; use ingest|detectors|diagnose")
    }
    Ok(())
}
