use crate::detectors;
use crate::featurizer;
use crate::ir::*;
use crate::redaction::excerpt;
use crate::store::Store;
use crate::util::{hash64, stable_id};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use reqwest::Client;
use schemars::schema_for;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Clone)]
pub struct Brain { store: Store, client: Client, api_key: Option<String>, app: Option<AppHandle> }
impl Brain { pub fn new(store:Store)->Self{let timeout_secs=std::env::var("WARDEN_FUGU_TIMEOUT_SECS").ok().and_then(|s|s.parse::<u64>().ok()).unwrap_or(75); let client=Client::builder().connect_timeout(Duration::from_secs(10)).timeout(Duration::from_secs(timeout_secs)).build().unwrap_or_else(|_|Client::new()); Self{store,client,api_key:std::env::var("SAKANA_API_KEY").ok(),app:None}} pub fn with_app(mut self, app:AppHandle)->Self{self.app=Some(app); self} pub fn available(&self)->bool{self.api_key.as_ref().is_some_and(|s|!s.trim().is_empty())} }

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct DiagnosticianOutput { findings: Vec<FuguFinding> }
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct FuguFinding { pattern_id:String, severity:u8, frequency:f64, est_cost_tokens:u64, est_cost_minutes:u64, confidence:f64, rationale:String, evidence:Vec<FuguEvidence> }
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct FuguEvidence { session:String, turn:Option<String>, event:Option<String>, quote:Option<String> }
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct CoachOutput { ranked_holes:Vec<String>, do_items:Vec<String>, stop_items:Vec<String>, narrative:String }
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct VerifyOutput { refuted:bool, confidence:f64, verdict:String }

impl Brain {
    pub async fn run_pipeline(&self, scope:RunScope) -> Result<Diagnosis> {
        let profile=featurizer::compute_all(&self.store)?;
        let candidates=detectors::nominate(&self.store,&profile)?;
        if candidates.is_empty(){ let d=Diagnosis{id:stable_id(&["diagnosis","empty",&Utc::now().to_rfc3339()]),created_at:Utc::now(),ranked_findings:vec![],do_items:vec!["Keep generating more transcript data; no deterministic hole crossed threshold yet.".into()],stop_items:vec![],narrative:"WARDEN ingested Claude Code sessions but no high-confidence recurring hole crossed the detector thresholds.".into(),detector_only:true}; self.store.save_diagnosis(&d)?; return Ok(d); }
        let Some(_) = self.api_key else { let d=detector_only_diagnosis(candidates,"SAKANA_API_KEY is not set; showing deterministic detector-only diagnosis."); self.store.save_findings(&d.ranked_findings)?; self.store.save_diagnosis(&d)?; return Ok(d); };
        let diagnosed = match self.diagnose(&profile,&candidates).await { Ok(f)=>f, Err(e)=>{ let d=detector_only_diagnosis(candidates, &format!("Fugu diagnosis failed ({e}); showing deterministic detector-only diagnosis.")); self.store.save_findings(&d.ranked_findings)?; self.store.save_diagnosis(&d)?; return Ok(d); } };
        let coached = self.coach(&profile,&diagnosed).await.unwrap_or_else(|_| CoachOutput{ranked_holes:diagnosed.iter().map(|f|f.id.clone()).collect(),do_items:vec!["Move search/discovery into delegated subagents before reading large file sets.".into(),"Require an explicit verification command before marking agent work complete.".into()],stop_items:vec!["Stop letting main-context searches accumulate before the first implementation edit.".into()],narrative:"Fugu confirmed recurring workflow holes in the Claude Code transcript set.".into()});
        let mut verified=Vec::new();
        for mut f in diagnosed.into_iter().take(6) { if f.severity>=4 { match self.verify(&f).await { Ok(v) if !v.refuted => { f.confidence=(f.confidence*v.confidence).min(0.99); f.status="confirmed".into(); f.verifier_verdict=Some(v.verdict); verified.push(f); }, Ok(v)=>tracing::info!(pattern=%f.pattern_id, verdict=%v.verdict, "finding refuted"), Err(e)=>{ f.status="confirmed".into(); f.verifier_verdict=Some(format!("Verifier unavailable; retained detector+diagnostician finding: {e}")); verified.push(f); } } } else { f.status="confirmed".into(); verified.push(f); } }
        if verified.is_empty(){ verified=detectors::nominate(&self.store,&profile)?; }
        let ranked=rank_by_coach(verified, &coached.ranked_holes);
        let d=Diagnosis{id:stable_id(&["diagnosis",&Utc::now().to_rfc3339(),&format!("{:?}",scope.query)]),created_at:Utc::now(),ranked_findings:ranked,do_items:coached.do_items,stop_items:coached.stop_items,narrative:coached.narrative,detector_only:false};
        self.store.save_findings(&d.ranked_findings)?; self.store.save_diagnosis(&d)?; Ok(d)
    }
    async fn diagnose(&self, profile:&CompetenceProfile, candidates:&[Finding]) -> Result<Vec<Finding>> {
        let input=json!({"task":"Diagnose recurring holes in this operator's Claude Code agent workflow. Confirm only findings supported by evidence. Return evidence-cited JSON.","profile":profile,"candidate_findings":candidates.iter().map(compact_finding).collect::<Vec<_>>()});
        let out:DiagnosticianOutput=self.call_json("diagnostician","fugu-ultra","xhigh", serde_json::to_value(schema_for!(DiagnosticianOutput))?, &input).await?;
        let mut result=Vec::new();
        for ff in out.findings { if let Some(base)=candidates.iter().find(|c|c.pattern_id==ff.pattern_id) { let mut f=base.clone(); f.severity=ff.severity.clamp(1,5); f.frequency=ff.frequency.clamp(0.0,1.0); f.est_cost_tokens=ff.est_cost_tokens; f.est_cost_minutes=ff.est_cost_minutes; f.confidence=ff.confidence.clamp(0.0,1.0); f.rationale=ff.rationale; if !ff.evidence.is_empty(){ f.evidence=ff.evidence.into_iter().map(|e|EvidenceRef{session_id:e.session,turn_id:e.turn,event_id:e.event,quote:e.quote.map(|q|excerpt(&q,220)),source_path:None}).collect(); } result.push(f); } }
        if result.is_empty(){ Ok(candidates.to_vec()) } else { Ok(result) }
    }
    async fn coach(&self, profile:&CompetenceProfile, findings:&[Finding]) -> Result<CoachOutput> {
        let input=json!({"task":"Turn verified workflow holes into terse, practical coaching for a founder using coding agents. Be direct. Do not invent fixes beyond evidence.","profile":profile,"findings":findings});
        self.call_json("coach","fugu-ultra","xhigh", serde_json::to_value(schema_for!(CoachOutput))?, &input).await
    }
    async fn verify(&self, finding:&Finding) -> Result<VerifyOutput> {
        let input=json!({"task":"Adversarially try to refute this workflow diagnosis. If evidence is weak or alternative explanations dominate, set refuted=true. Otherwise explain why it survives.","finding":finding});
        self.call_json("verifier","fugu","high", serde_json::to_value(schema_for!(VerifyOutput))?, &input).await
    }
    async fn call_json<T: for<'de> Deserialize<'de>>(&self, stage:&str, model:&str, effort:&str, schema:Value, input:&Value) -> Result<T> {
        let key=self.api_key.as_ref().context("SAKANA_API_KEY missing")?;
        let prompt=serde_json::to_string(input)?;
        let req_hash=hex::encode(hash64(prompt.as_bytes()).to_be_bytes());
        let body=json!({"model":model,"input":[{"role":"system","content":[{"type":"input_text","text":"You are WARDEN's analysis brain. Return only valid JSON matching the schema. Cite only supplied evidence."}]},{"role":"user","content":[{"type":"input_text","text":prompt}]}],"text":{"format":{"type":"json_schema","name":stage,"schema":schema,"strict":false}},"reasoning":{"effort":effort},"stream":true,"stream_options":{"include_usage":true},"max_output_tokens":12000});
        let start=Instant::now(); let resp=self.client.post("https://api.sakana.ai/v1/responses").bearer_auth(key).json(&body).send().await?;
        if !resp.status().is_success(){ let status=resp.status(); let text=resp.text().await.unwrap_or_default(); return Err(anyhow!("Fugu {stage} HTTP {status}: {}", text.chars().take(800).collect::<String>())); }
        let mut stream=resp.bytes_stream(); let mut buf=String::new(); let mut out=String::new(); let mut usage=json!({});
        while let Some(chunk)=stream.next().await { let chunk=chunk?; buf.push_str(&String::from_utf8_lossy(&chunk)); while let Some(pos)=buf.find("\n\n") { let frame=buf[..pos].to_string(); buf=buf[pos+2..].to_string(); for line in frame.lines() { let Some(data)=line.strip_prefix("data: ") else { continue; }; if data.trim()=="[DONE]"{continue;} let Ok(v)=serde_json::from_str::<Value>(data) else {continue;}; match v.get("type").and_then(Value::as_str).unwrap_or("") { "response.output_text.delta" => { if let Some(delta)=v.get("delta").and_then(Value::as_str){ out.push_str(delta); self.emit_delta(stage,delta); } }, "response.completed" => { if let Some(u)=v.get("response").and_then(|r|r.get("usage")){ usage=u.clone(); } if out.trim().is_empty(){ out=extract_output_text(v.get("response").unwrap_or(&Value::Null)); } }, _ => {} } } } }
        if out.trim().is_empty(){ return Err(anyhow!("Fugu {stage} returned empty output")); }
        let parsed_text=repair_json_text(&out); let val:Value=serde_json::from_str(&parsed_text).with_context(||format!("parse Fugu {stage} JSON: {parsed_text}"))?;
        let u=usage_tokens(&usage); self.emit_usage(stage,&u); self.store.save_fugu_run(stage,model,effort,&req_hash,u.0,u.1,u.2,u.3,start.elapsed().as_millis() as u64)?;
        Ok(serde_json::from_value(val)?)
    }
    fn emit_delta(&self, stage:&str, delta:&str){ if let Some(app)=&self.app { let _=app.emit("fugu_delta", json!({"stage":stage,"delta":delta})); } }
    fn emit_usage(&self, stage:&str, u:&(u64,u64,u64,u64)){ if let Some(app)=&self.app { let _=app.emit("fugu_usage", json!({"stage":stage,"input_tokens":u.0,"output_tokens":u.1,"orchestration_input_tokens":u.2,"orchestration_output_tokens":u.3})); } }
}
fn compact_finding(f:&Finding)->Value{ json!({"id":f.id,"pattern_id":f.pattern_id,"title":f.title,"severity":f.severity,"frequency":f.frequency,"est_cost_tokens":f.est_cost_tokens,"est_cost_minutes":f.est_cost_minutes,"confidence":f.confidence,"rationale":f.rationale,"evidence":f.evidence.iter().map(|e|json!({"session_id":e.session_id,"turn_id":e.turn_id,"event_id":e.event_id,"quote":e.quote.as_ref().map(|q|excerpt(q,260)),"source_path":e.source_path})).collect::<Vec<_>>()}) }
fn detector_only_diagnosis(findings:Vec<Finding>, narrative:&str)->Diagnosis{ Diagnosis{id:stable_id(&["detector-only",&Utc::now().to_rfc3339()]),created_at:Utc::now(),ranked_findings:findings,do_items:vec!["Delegate broad search/reading into subagents before main-context implementation.".into(),"Add an explicit test/build verification requirement to agent prompts.".into(),"Move repeated project context into durable CLAUDE.md guidance.".into()],stop_items:vec!["Stop accepting 'done' without a recorded verification command.".into(),"Stop spending main context on exploratory file inventory.".into()],narrative:narrative.into(),detector_only:true} }
fn rank_by_coach(mut findings:Vec<Finding>, ranked:&[String])->Vec<Finding>{ findings.sort_by_key(|f| ranked.iter().position(|id|id==&f.id||id==&f.pattern_id).unwrap_or(999)); findings.truncate(3); findings }
fn extract_output_text(response:&Value)->String{ let mut s=String::new(); if let Some(arr)=response.get("output").and_then(Value::as_array){ for item in arr { if let Some(ca)=item.get("content").and_then(Value::as_array){ for c in ca { if c.get("type").and_then(Value::as_str)==Some("output_text") { if let Some(t)=c.get("text").and_then(Value::as_str){ s.push_str(t); } } } } } } s }
fn repair_json_text(s:&str)->String{ let t=s.trim(); if t.starts_with('{')||t.starts_with('['){ t.to_string() } else if let Some(i)=t.find('{'){ t[i..].to_string() } else { t.to_string() } }
fn usage_tokens(u:&Value)->(u64,u64,u64,u64){ (u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),u.pointer("/input_tokens_details/orchestration_input_tokens").and_then(Value::as_u64).unwrap_or(0),u.pointer("/output_tokens_details/orchestration_output_tokens").and_then(Value::as_u64).unwrap_or(0)) }

#[cfg(test)] mod tests { use super::*; #[test] fn extracts_split_output(){ let v=json!({"output":[{"type":"message","content":[{"type":"output_text","text":"{\"ok\":"}]},{"type":"message","content":[{"type":"output_text","text":"true}"}]}]}); assert_eq!(extract_output_text(&v),"{\"ok\":true}"); } }
