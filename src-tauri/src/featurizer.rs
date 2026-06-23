use crate::ir::*;
use crate::store::Store;
use anyhow::Result;
use std::collections::{HashMap, HashSet};

pub const FEATURIZER_VERSION: &str = "warden-m0-v1";
const DEFAULT_WINDOW: f64 = 200_000.0;

pub fn compute_all(store:&Store) -> Result<CompetenceProfile> {
    let sessions=store.sessions()?;
    for s in &sessions { let ev=store.session_events(&s.id)?; let f=compute_session(s, &ev); store.save_feature(&f, FEATURIZER_VERSION)?; }
    let features=store.all_features()?; let mut profile=aggregate_profile(store, &features)?; let (sc,ec,fc)=store.counts()?; profile.session_count=sc; profile.event_count=ec; profile.finding_count=fc; store.save_profile(&profile)?; Ok(profile)
}

pub fn compute_session(session:&Session, rows:&[(Turn,EventRecord)]) -> FeatureVector {
    let mut f=FeatureVector{ session_id:session.id.clone(), started_at:Some(session.started_at), project:session.project.as_ref().map(|p|p.cwd.display().to_string()), ..Default::default() };
    let mut tool_results=0u32; let mut tool_errors=0u32; let mut thinking_tokens=0u64; let mut output_tokens=0u64; let mut input_total=0u64; let mut cache_read=0u64; let mut cache_create=0u64; let mut cumulative_input=0u64; let mut search_heavy_turns=HashSet::new(); let mut user_prompts:Vec<String>=Vec::new(); let mut previous_user=false; let mut file_counts=HashMap::<String,u32>::new(); let mut command_counts=HashMap::<String,u32>::new(); let mut permission_modes=HashMap::<String,u32>::new(); let mut saw_edit=false; let mut saw_done=false;
    for (turn, rec) in rows {
        match &rec.event {
            Event::UserPrompt{text,is_meta,..} => { if !*is_meta { user_prompts.push(text.clone()); if previous_user { f.reprompt_count+=1; } previous_user=true; } },
            Event::AssistantText{text} => { previous_user=false; output_tokens += (text.len()/4) as u64; let l=text.to_lowercase(); if l.contains("done")||l.contains("complete")||l.contains("fixed") { saw_done=true; } },
            Event::Thinking{tokens} => thinking_tokens += *tokens as u64,
            Event::ToolCall{tool,input,kind,..} => { previous_user=false; f.tool_call_count+=1; let tl=tool.to_ascii_lowercase(); let is_search=matches!(tl.as_str(),"grep"|"glob"|"read"|"ls") || tl.contains("grep") || tl.contains("read"); if is_search && !turn.is_sidechain { f.search_in_main_context+=1; search_heavy_turns.insert(turn.id.clone()); } if matches!(kind, ToolKind::SubagentTask) || tool=="Task" { f.subagent_spawn_count+=1; } if tl=="bash" { if let Some(cmd)=input.get("command").and_then(|v|v.as_str()) { let key=cmd.split_whitespace().take(4).collect::<Vec<_>>().join(" "); *command_counts.entry(key).or_default()+=1; let cl=cmd.to_lowercase(); if cl.contains("test")||cl.contains("cargo build")||cl.contains("pnpm build")||cl.contains("npm test")||cl.contains("pytest") { f.verification_present=true; } } } },
            Event::ToolResult{status,..} => { tool_results+=1; if *status==ToolStatus::Error { tool_errors+=1; } },
            Event::TokenUsage{input,output,cache_creation,cache_read:cr,..} => { input_total+=*input as u64; output_tokens+=*output as u64; cache_read+=*cr as u64; cache_create+=*cache_creation as u64; cumulative_input+=*input as u64; f.context_saturation_peak=f.context_saturation_peak.max(cumulative_input as f64 / DEFAULT_WINDOW); },
            Event::FileSnapshot{files} => { saw_edit=true; for file in files { *file_counts.entry(file.path.clone()).or_default()+=1; } },
            Event::ModeChange{mode} => { *permission_modes.entry(mode.clone()).or_default()+=1; },
            _=>{}
        }
    }
    if saw_done && saw_edit && !f.verification_present { /* signal retained by false */ }
    f.token_burn_total=input_total+output_tokens+cache_create;
    f.cache_read_ratio= if input_total+cache_create+cache_read>0 { cache_read as f64/(input_total+cache_create+cache_read) as f64 } else { 0.0 };
    f.tool_error_rate= if tool_results>0 { tool_errors as f64/tool_results as f64 } else { 0.0 };
    f.ignored_error_count=tool_errors.saturating_sub((tool_errors>0 && f.verification_present) as u32);
    f.subagent_delegation_rate= if !search_heavy_turns.is_empty() { f.subagent_spawn_count as f64/search_heavy_turns.len() as f64 } else if f.tool_call_count>0 { f.subagent_spawn_count as f64/f.tool_call_count as f64 } else { 0.0 };
    f.prompt_specificity=prompt_specificity(&user_prompts);
    f.file_churn= if file_counts.is_empty(){0.0}else{file_counts.values().map(|v|*v as f64).sum::<f64>()/file_counts.len() as f64};
    f.thrash_index=file_counts.values().filter(|v|**v>=3).count() as f64 + command_counts.values().filter(|v|**v>=3).count() as f64;
    f.planning_ratio= if thinking_tokens+output_tokens>0 { thinking_tokens as f64/(thinking_tokens+output_tokens) as f64 } else { 0.0 };
    f.permission_friction=permission_modes.values().filter(|v|**v>=2).sum();
    f
}

fn prompt_specificity(prompts:&[String])->f64{ if prompts.is_empty(){return 0.0;} let mut scores=Vec::new(); for p in prompts { let words=p.split_whitespace().count() as f64; let has_path=p.contains('/')||p.contains(".rs")||p.contains(".ts")||p.contains(".py")||p.contains(".md"); let has_accept=["verify","test","run","acceptance","done","must","complete","fix"].iter().filter(|w|p.to_lowercase().contains(*w)).count(); let concrete=p.matches('`').count().min(4) as f64/4.0; let s=(words/80.0).min(0.42)+if has_path{0.22}else{0.0}+(has_accept as f64*0.08).min(0.24)+concrete*0.12; scores.push(s.min(1.0)); } scores.iter().sum::<f64>()/scores.len() as f64 }

fn aggregate_profile(_store:&Store, features:&[FeatureVector]) -> Result<CompetenceProfile> { let n=features.len().max(1) as f64; let mut p=CompetenceProfile::default(); p.token_burn_total=features.iter().map(|f|f.token_burn_total).sum(); p.avg_prompt_specificity=features.iter().map(|f|f.prompt_specificity).sum::<f64>()/n; p.avg_cache_read_ratio=features.iter().map(|f|f.cache_read_ratio).sum::<f64>()/n; p.avg_tool_error_rate=features.iter().map(|f|f.tool_error_rate).sum::<f64>()/n; p.no_delegation_sessions=features.iter().filter(|f|f.tool_call_count>=8 && f.subagent_spawn_count==0).count() as u32; p.context_bloat_sessions=features.iter().filter(|f|f.search_in_main_context>=6).count() as u32; p.unverified_sessions=features.iter().filter(|f|f.tool_call_count>=3 && !f.verification_present).count() as u32; p.repeated_explanation_clusters=repeated_clusters(features); Ok(p) }
fn repeated_clusters(features:&[FeatureVector])->Vec<RepeatedCluster>{ let mut by_project=HashMap::<String,Vec<String>>::new(); for f in features { if let Some(p)=&f.project { by_project.entry(p.clone()).or_default().push(f.session_id.clone()); } } let mut clusters:Vec<_>=by_project.into_iter().filter(|(_,v)|v.len()>=3).map(|(k,v)|RepeatedCluster{phrase:format!("Repeated work in {k}"),count:v.len() as u32,session_ids:v.into_iter().take(10).collect()}).collect(); clusters.sort_by_key(|c| std::cmp::Reverse(c.count)); clusters.truncate(8); clusters }

#[cfg(test)] mod tests { use super::*; use chrono::Utc; use std::path::PathBuf; fn sess()->Session{Session{id:"s".into(),harness:Harness::ClaudeCode,external_id:"e".into(),project:None,model_ids:vec![],started_at:Utc::now(),ended_at:None,source_path:PathBuf::from("x"),raw_hash:1,ingested_at:Utc::now(),meta:serde_json::Value::Null}} #[test] fn computes_search_and_no_verification(){ let s=sess(); let t=Turn{id:"t".into(),session_id:"s".into(),parent_id:None,role:Role::Assistant,index:0,started_at:Utc::now(),duration_ms:None,is_sidechain:false}; let rows=(0..7).map(|i|(t.clone(),EventRecord{id:i.to_string(),turn_id:"t".into(),session_id:"s".into(),ts:Utc::now(),event:Event::ToolCall{tool:"Read".into(),input:serde_json::Value::Null,call_id:i.to_string(),kind:ToolKind::Builtin},raw_ref:RawRef{source_path:PathBuf::from("x"),offset:0,line:0}})).collect::<Vec<_>>(); let f=compute_session(&s,&rows); assert_eq!(f.search_in_main_context,7); assert_eq!(f.subagent_spawn_count,0); } }
