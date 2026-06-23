use super::{Adapter, SessionBatch};
use crate::ir::*;
use crate::store::Store;
use crate::util::{default_claude_projects, hash64, parse_ts, repo_root, stable_id, truncate_chars};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct ClaudeCodeAdapter { pub root: PathBuf, pub store: Store, pub max_files: Option<usize> }
impl ClaudeCodeAdapter { pub fn new(store: Store) -> Self { Self{ root: default_claude_projects(), store, max_files: None } } pub fn with_root(root:PathBuf, store:Store)->Self{Self{root,store,max_files:None}} }
impl Adapter for ClaudeCodeAdapter {
    fn harness(&self)->Harness{Harness::ClaudeCode}
    fn detect(&self)->Result<Vec<PathBuf>>{
        if !self.root.exists(){ return Ok(vec![]); }
        let mut paths:Vec<PathBuf>=WalkDir::new(&self.root).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file() && e.path().extension().map(|x|x=="jsonl").unwrap_or(false)).map(|e|e.into_path()).collect();
        paths.sort_by_key(|p| std::fs::metadata(p).and_then(|m|m.modified()).ok()); paths.reverse();
        if let Some(n)=self.max_files { paths.truncate(n); }
        Ok(paths)
    }
    fn backfill(&self)->Result<Vec<SessionBatch>>{
        let mut out=Vec::new();
        for p in self.detect()? { let bytes=std::fs::read(&p).with_context(||format!("read {}",p.display()))?; let raw_hash=hash64(&bytes); if self.store.source_raw_hash(&p)?.is_some_and(|h|h==raw_hash){ continue; } match parse_file(&p,&bytes,raw_hash){ Ok(b)=>out.push(b), Err(e)=>tracing::warn!(path=%p.display(), error=?e, "skipping malformed Claude transcript") } }
        Ok(out)
    }
}

pub fn ingest_all(store:&Store, root:Option<PathBuf>, max_files:Option<usize>) -> Result<(usize,usize)> {
    let mut a=ClaudeCodeAdapter::with_root(root.unwrap_or_else(default_claude_projects), store.clone()); a.max_files=max_files;
    let batches=a.backfill()?; let mut sessions=0; let mut events=0;
    for b in batches { events+=b.events.len(); store.upsert_session_batch(&b.session,&b.turns,&b.events,b.offset)?; sessions+=1; }
    Ok((sessions,events))
}

fn parse_file(path:&Path, bytes:&[u8], raw_hash:u64) -> Result<SessionBatch> {
    let f=File::open(path)?; let mut reader=BufReader::new(f); let mut buf=String::new(); let mut offset=0u64; let mut line_no=0u32;
    let mut raw_records:Vec<(u64,u32,Value)>=Vec::new();
    loop { buf.clear(); let n=reader.read_line(&mut buf)?; if n==0{break;} line_no+=1; let line=buf.trim_end_matches(['\n','\r']); if !line.trim().is_empty(){ if let Ok(v)=serde_json::from_str::<Value>(line){ raw_records.push((offset,line_no,v)); } } offset+=n as u64; }
    let first=raw_records.first().map(|(_,_,v)|v).context("empty jsonl")?;
    let external_id=first.get("sessionId").and_then(Value::as_str).map(str::to_string).or_else(|| path.file_stem().map(|s|s.to_string_lossy().to_string())).unwrap_or_else(|| stable_id(&[&path.to_string_lossy()]));
    let sid=stable_id(&["claude_code", &external_id, &path.to_string_lossy()]);
    let mut turns=Vec::new(); let mut events=Vec::new(); let mut models=BTreeSet::new(); let mut started=None; let mut ended=None; let mut project=None; let mut idx=0u32; let mut uuid_to_turn=HashMap::<String,String>::new(); let mut duration_by_parent=HashMap::<String,u64>::new(); let mut meta=json!({"ignored_record_types":{}});
    for (off, ln, v) in &raw_records {
        let ts=parse_ts(v.get("timestamp")); if started.map(|s| ts<s).unwrap_or(true){started=Some(ts)}; if ended.map(|e| ts>e).unwrap_or(true){ended=Some(ts)};
        if project.is_none() { if let Some(cwd)=v.get("cwd").and_then(Value::as_str) { let cwdp=PathBuf::from(cwd); project=Some(ProjectRef{ cwd:cwdp.clone(), repo_root:repo_root(&cwdp), git_branch:v.get("gitBranch").and_then(Value::as_str).map(str::to_string) }); } }
        match v.get("type").and_then(Value::as_str).unwrap_or("unknown") {
            "user"|"assistant" => {
                idx+=1; let uuid=v.get("uuid").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| stable_id(&[&sid,&idx.to_string(),&ln.to_string()])); let tid=stable_id(&[&sid,&uuid]); uuid_to_turn.insert(uuid.clone(),tid.clone());
                let role=if v.get("type").and_then(Value::as_str)==Some("user"){Role::User}else{Role::Assistant};
                let parent=v.get("parentUuid").and_then(Value::as_str).and_then(|p| uuid_to_turn.get(p).cloned().or_else(||Some(stable_id(&[&sid,p]))));
                let dur=duration_by_parent.remove(&uuid);
                turns.push(Turn{id:tid.clone(),session_id:sid.clone(),parent_id:parent,role:role.clone(),index:idx,started_at:ts,duration_ms:dur,is_sidechain:v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false)});
                let raw=RawRef{source_path:path.to_path_buf(),offset:*off,line:*ln};
                if role==Role::User { map_user(&mut events,&sid,&tid,ts,raw,v); } else { map_assistant(&mut events,&sid,&tid,ts,raw.clone(),v,&mut models); if let Some(src)=v.get("sourceToolAssistantUuid").or_else(||v.get("sourceToolAssistantUUID")).and_then(Value::as_str){ events.push(EventRecord{id:stable_id(&[&tid,"spawn",src]),turn_id:tid.clone(),session_id:sid.clone(),ts,event:Event::SubagentSpawn{source_assistant_uuid:src.to_string(),child_session:None},raw_ref:raw}); } }
            }
            "system" => { if v.get("subtype").and_then(Value::as_str)==Some("turn_duration") { if let (Some(parent),Some(d))=(v.get("parentUuid").and_then(Value::as_str), v.get("durationMs").and_then(Value::as_u64)) { duration_by_parent.insert(parent.to_string(),d); } }
                idx+=1; let tid=stable_id(&[&sid,"system",&ln.to_string()]); turns.push(Turn{id:tid.clone(),session_id:sid.clone(),parent_id:None,role:Role::System,index:idx,started_at:ts,duration_ms:None,is_sidechain:v.get("isSidechain").and_then(Value::as_bool).unwrap_or(false)}); events.push(EventRecord{id:stable_id(&[&tid,"notice"]),turn_id:tid,session_id:sid.clone(),ts,event:Event::SystemNotice{subtype:v.get("subtype").and_then(Value::as_str).unwrap_or("system").to_string(),data:v.clone()},raw_ref:RawRef{source_path:path.to_path_buf(),offset:*off,line:*ln}}); }
            "file-history-snapshot" => { idx+=1; let tid=stable_id(&[&sid,"snapshot",&ln.to_string()]); turns.push(Turn{id:tid.clone(),session_id:sid.clone(),parent_id:None,role:Role::System,index:idx,started_at:ts,duration_ms:None,is_sidechain:false}); events.push(EventRecord{id:stable_id(&[&tid,"files"]),turn_id:tid,session_id:sid.clone(),ts,event:Event::FileSnapshot{files:parse_snapshot(v.get("snapshot"))},raw_ref:RawRef{source_path:path.to_path_buf(),offset:*off,line:*ln}}); }
            "mode"|"permission-mode" => { idx+=1; let tid=stable_id(&[&sid,"mode",&ln.to_string()]); turns.push(Turn{id:tid.clone(),session_id:sid.clone(),parent_id:None,role:Role::System,index:idx,started_at:ts,duration_ms:None,is_sidechain:false}); let mode=v.get("mode").or_else(||v.get("permissionMode")).and_then(Value::as_str).unwrap_or("unknown").to_string(); events.push(EventRecord{id:stable_id(&[&tid,"mode"]),turn_id:tid,session_id:sid.clone(),ts,event:Event::ModeChange{mode},raw_ref:RawRef{source_path:path.to_path_buf(),offset:*off,line:*ln}}); }
            other => { let obj=meta.get_mut("ignored_record_types").unwrap().as_object_mut().unwrap(); let n=obj.get(other).and_then(Value::as_u64).unwrap_or(0)+1; obj.insert(other.to_string(), json!(n)); }
        }
    }
    Ok(SessionBatch{session:Session{id:sid,harness:Harness::ClaudeCode,external_id,project,model_ids:models.into_iter().collect(),started_at:started.unwrap_or_else(Utc::now),ended_at:ended,source_path:path.to_path_buf(),raw_hash,ingested_at:Utc::now(),meta},turns,events,offset})
}
fn map_user(events:&mut Vec<EventRecord>, sid:&str, tid:&str, ts:chrono::DateTime<Utc>, raw:RawRef, v:&Value){ let msg=&v["message"]; let content=&msg["content"]; let is_meta=v.get("isMeta").and_then(Value::as_bool).unwrap_or(false); if let Some(s)=content.as_str(){ events.push(EventRecord{id:stable_id(&[tid,"prompt"]),turn_id:tid.to_string(),session_id:sid.to_string(),ts,event:Event::UserPrompt{text:s.to_string(),attachments:vec![],is_meta},raw_ref:raw}); } else if let Some(arr)=content.as_array(){ let mut prompt_parts=Vec::new(); for (i,b) in arr.iter().enumerate(){ match b.get("type").and_then(Value::as_str){ Some("tool_result")=>{ let text=block_text(b.get("content")); let status=if b.get("is_error").and_then(Value::as_bool).unwrap_or(false){ToolStatus::Error}else{ToolStatus::Ok}; events.push(EventRecord{id:stable_id(&[tid,"tool_result",&i.to_string()]),turn_id:tid.to_string(),session_id:sid.to_string(),ts,event:Event::ToolResult{call_id:b.get("tool_use_id").and_then(Value::as_str).unwrap_or("unknown").to_string(),status,bytes:text.len() as u64,summary:Some(truncate_chars(&text,500))},raw_ref:raw.clone()}); }, Some("text")=>prompt_parts.push(block_text(b.get("text"))), _=>{} } } if !prompt_parts.is_empty(){ events.push(EventRecord{id:stable_id(&[tid,"prompt"]),turn_id:tid.to_string(),session_id:sid.to_string(),ts,event:Event::UserPrompt{text:prompt_parts.join("\n"),attachments:vec![],is_meta},raw_ref:raw}); } } }
fn map_assistant(events:&mut Vec<EventRecord>, sid:&str, tid:&str, ts:chrono::DateTime<Utc>, raw:RawRef, v:&Value, models:&mut BTreeSet<String>){ let msg=&v["message"]; if let Some(m)=msg.get("model").and_then(Value::as_str){models.insert(m.to_string());} if let Some(arr)=msg.get("content").and_then(Value::as_array){ for (i,b) in arr.iter().enumerate(){ match b.get("type").and_then(Value::as_str){ Some("text")=>events.push(EventRecord{id:stable_id(&[tid,"text",&i.to_string()]),turn_id:tid.to_string(),session_id:sid.to_string(),ts,event:Event::AssistantText{text:block_text(b.get("text"))},raw_ref:raw.clone()}), Some("thinking")=>{let text=block_text(b.get("thinking").or_else(||b.get("text"))); events.push(EventRecord{id:stable_id(&[tid,"thinking",&i.to_string()]),turn_id:tid.to_string(),session_id:sid.to_string(),ts,event:Event::Thinking{tokens:(text.len()/4) as u32},raw_ref:raw.clone()});}, Some("tool_use")|Some("server_tool_use")=>{let name=b.get("name").and_then(Value::as_str).unwrap_or("unknown"); let kind=if name=="Task"{ToolKind::SubagentTask}else if b.get("type").and_then(Value::as_str)==Some("server_tool_use"){ToolKind::Mcp}else{ToolKind::Builtin}; events.push(EventRecord{id:stable_id(&[tid,"tool",&i.to_string()]),turn_id:tid.to_string(),session_id:sid.to_string(),ts,event:Event::ToolCall{tool:name.to_string(),input:b.get("input").cloned().unwrap_or(Value::Null),call_id:b.get("id").and_then(Value::as_str).unwrap_or("unknown").to_string(),kind},raw_ref:raw.clone()});}, _=>{} } } } if let Some(u)=msg.get("usage"){ let model=msg.get("model").and_then(Value::as_str).unwrap_or("unknown").to_string(); events.push(EventRecord{id:stable_id(&[tid,"usage"]),turn_id:tid.to_string(),session_id:sid.to_string(),ts,event:Event::TokenUsage{input:u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,output:u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,cache_creation:u.get("cache_creation_input_tokens").and_then(Value::as_u64).or_else(||u.get("cache_creation").and_then(Value::as_u64)).unwrap_or(0) as u32,cache_read:u.get("cache_read_input_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,model,orchestration:None},raw_ref:raw}); } }
fn block_text(v:Option<&Value>)->String{ match v { Some(Value::String(s))=>s.clone(), Some(Value::Array(a))=>a.iter().map(|x| block_text(Some(x))).collect::<Vec<_>>().join("\n"), Some(Value::Object(o))=>o.get("text").and_then(Value::as_str).map(str::to_string).unwrap_or_else(||Value::Object(o.clone()).to_string()), Some(x)=>x.to_string(), None=>String::new() } }
fn parse_snapshot(v:Option<&Value>)->Vec<FileEdit>{ match v { Some(Value::Object(o))=>o.keys().take(500).map(|k|FileEdit{path:k.clone(),old_hash:None,new_hash:None,lines_changed:None}).collect(), Some(Value::Array(a))=>a.iter().filter_map(|x|x.get("path").and_then(Value::as_str).map(|p|FileEdit{path:p.to_string(),old_hash:None,new_hash:None,lines_changed:None})).collect(), _=>vec![] } }

#[cfg(test)] mod tests { use super::*; use crate::store::Store; use tempfile::tempdir; #[test] fn parses_minimal_claude_jsonl(){ let dir=tempdir().unwrap(); let p=dir.path().join("s.jsonl"); std::fs::write(&p, r#"{"type":"user","uuid":"u1","sessionId":"s","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"fix tests"},"cwd":"/tmp","gitBranch":"main"}
{"type":"assistant","uuid":"a1","parentUuid":"u1","sessionId":"s","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","model":"claude","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cargo test"}},{"type":"text","text":"done"}],"usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":2}}}
"#).unwrap(); let bytes=std::fs::read(&p).unwrap(); let b=parse_file(&p,&bytes,hash64(&bytes)).unwrap(); assert_eq!(b.turns.len(),2); assert!(b.events.iter().any(|e| matches!(e.event,Event::ToolCall{ref tool,..} if tool=="Bash"))); assert_eq!(b.session.model_ids, vec!["claude".to_string()]); }
#[test] fn ingest_is_idempotent_by_hash(){ let dir=tempdir().unwrap(); let root=dir.path().join("projects"); std::fs::create_dir_all(&root).unwrap(); let p=root.join("s.jsonl"); std::fs::write(&p,"{\"type\":\"user\",\"uuid\":\"u\",\"sessionId\":\"s\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"content\":\"hello\"}}\n").unwrap(); let store=Store::memory().unwrap(); assert_eq!(ingest_all(&store,Some(root.clone()),None).unwrap().0,1); assert_eq!(ingest_all(&store,Some(root),None).unwrap().0,0); } }
