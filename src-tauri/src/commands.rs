use crate::brain::Brain;
use crate::featurizer;
use crate::ingest::claude_code;
use crate::ir::*;
use crate::scaffold::{not_in_slice, AgentInstance};
use crate::store::Store;
use crate::util::default_db_path;
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState { pub store: Store, pub run_lock: Arc<Mutex<()>> }
impl AppState { pub fn init() -> Result<Self> { let store=Store::open(default_db_path())?; Ok(Self{store,run_lock:Arc::new(Mutex::new(()))}) } }

#[tauri::command]
pub async fn query_profile(state: tauri::State<'_, AppState>) -> Result<CompetenceProfile, String> { let mut p=state.store.profile().map_err(|e|e.to_string())?; let (s,e,f)=state.store.counts().map_err(|e|e.to_string())?; p.session_count=s; p.event_count=e; p.finding_count=f; Ok(p) }
#[tauri::command]
pub async fn get_diagnosis(state: tauri::State<'_, AppState>) -> Result<Option<Diagnosis>, String> { state.store.latest_diagnosis().map_err(|e|e.to_string()) }
#[tauri::command]
pub async fn get_findings(state: tauri::State<'_, AppState>) -> Result<Vec<Finding>, String> { let p=state.store.profile().map_err(|e|e.to_string())?; crate::detectors::nominate(&state.store,&p).map_err(|e|e.to_string()) }
#[tauri::command]
pub async fn run_diagnosis(app: tauri::AppHandle, state: tauri::State<'_, AppState>, scope: RunScope) -> Result<Diagnosis, String> { let _guard=state.run_lock.lock().await; claude_code::ingest_all(&state.store,None,None).map_err(|e|e.to_string())?; featurizer::compute_all(&state.store).map_err(|e|e.to_string())?; Brain::new(state.store.clone()).with_app(app).run_pipeline(scope).await.map_err(|e|e.to_string()) }
#[tauri::command]
pub async fn ask(app: tauri::AppHandle, state: tauri::State<'_, AppState>, query:String, mode:Option<String>) -> Result<Diagnosis, String> { run_diagnosis(app,state,RunScope{harness:Some("claude_code".into()),query:Some(query),force:Some(mode.as_deref()==Some("force"))}).await }
#[tauri::command]
pub async fn apply_artifact(_id:String)->Result<(),String>{ Err(not_in_slice("Forge artifact apply")) }
#[tauri::command]
pub async fn revert_artifact(_id:String)->Result<(),String>{ Err(not_in_slice("Forge artifact revert")) }
#[tauri::command]
pub async fn start_voice()->Result<(),String>{ Err(not_in_slice("Voice STT")) }
#[tauri::command]
pub async fn stop_voice()->Result<(),String>{ Err(not_in_slice("Voice STT")) }
#[tauri::command]
pub async fn capture_screen()->Result<(),String>{ Err(not_in_slice("Screen Q&A")) }
#[tauri::command]
pub async fn set_config(_value:serde_json::Value)->Result<(),String>{ Err(not_in_slice("Config UI persistence")) }
#[tauri::command]
pub async fn mute_pattern(_id:String)->Result<(),String>{ Err(not_in_slice("Live interjection muting")) }
#[tauri::command]
pub async fn list_fleet()->Result<Vec<AgentInstance>,String>{ Err(not_in_slice("RADAR fleet tracker")) }
#[tauri::command]
pub async fn locate_agent(_id:String)->Result<(),String>{ Err(not_in_slice("RADAR locate")) }
#[tauri::command]
pub async fn warp_to_agent(_id:String)->Result<(),String>{ Err(not_in_slice("RADAR warp")) }
