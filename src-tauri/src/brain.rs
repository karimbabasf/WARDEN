use crate::detectors;
use crate::featurizer;
use crate::ir::*;
use crate::redaction::excerpt;
use crate::store::Store;
use crate::util::{brain_api_key, brain_diagnose_model, brain_effort, brain_responses_url,
                  brain_verify_model, hash64, stable_id};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use futures_util::StreamExt;
use reqwest::{header, Client};
use schemars::schema_for;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

#[derive(Clone)]
pub struct Brain {
    store: Store,
    client: Client,
    api_key: Option<String>,
    app: Option<AppHandle>,
}

impl Brain {
    pub fn new(store: Store) -> Self {
        let timeout_secs = std::env::var("WARDEN_FUGU_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(75);
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self {
            store,
            client,
            api_key: brain_api_key(),
            app: None,
        }
    }

    pub fn with_app(mut self, app: AppHandle) -> Self {
        self.app = Some(app);
        self
    }

    pub fn available(&self) -> bool {
        self.api_key.as_ref().is_some_and(|s| !s.trim().is_empty())
    }
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct DiagnosticianOutput {
    findings: Vec<FuguFinding>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct FuguFinding {
    pattern_id: String,
    severity: u8,
    frequency: f64,
    est_cost_tokens: u64,
    est_cost_minutes: u64,
    confidence: f64,
    rationale: String,
    evidence: Vec<FuguEvidence>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct FuguEvidence {
    session: String,
    turn: Option<String>,
    event: Option<String>,
    quote: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct CoachOutput {
    ranked_holes: Vec<String>,
    do_items: Vec<String>,
    stop_items: Vec<String>,
    narrative: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct VerifyOutput {
    refuted: bool,
    confidence: f64,
    verdict: String,
}

impl Brain {
    pub async fn run_pipeline(&self, scope: RunScope) -> Result<Diagnosis> {
        let profile = featurizer::compute_all(&self.store)?;
        let candidates = detectors::nominate(&self.store, &profile)?;
        if candidates.is_empty() {
            let d = Diagnosis {
                id: stable_id(&["diagnosis", "empty", &Utc::now().to_rfc3339()]),
                created_at: Utc::now(),
                ranked_findings: vec![],
                do_items: vec![
                    "Keep generating more transcript data; no deterministic hole crossed threshold yet."
                        .into(),
                ],
                stop_items: vec![],
                narrative: "WARDEN ingested Claude Code sessions but no high-confidence recurring hole crossed the detector thresholds."
                    .into(),
                detector_only: true,
            };
            self.store.save_diagnosis(&d)?;
            return Ok(d);
        }

        // Candidates exist: announce them to the war-room before any Fugu call so
        // nodes can spawn from real nominated holes. No-op when headless.
        self.emit_candidates(&candidates);

        let Some(_) = self.api_key else {
            let d = detector_only_diagnosis(
                candidates,
                "SAKANA_API_KEY is not set; showing deterministic detector-only diagnosis.",
            );
            self.store.save_findings(&d.ranked_findings)?;
            self.store.save_diagnosis(&d)?;
            return Ok(d);
        };

        let diagnosed = match self.diagnose(&profile, &candidates).await {
            Ok(f)
                if f.iter()
                    .filter(|finding| !is_detector_backfill(finding))
                    .count()
                    >= 3 =>
            {
                f
            }
            Ok(f) => {
                tracing::warn!(
                    confirmed = f.iter().filter(|finding| !is_detector_backfill(finding)).count(),
                    "full Fugu diagnosis returned fewer than three confirmed findings; retrying compact path"
                );
                self.diagnose_compact(&profile, &candidates).await?
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "full Fugu diagnosis failed; retrying compact lower-latency diagnosis path"
                );
                match self.diagnose_compact(&profile, &candidates).await {
                    Ok(f) => f,
                    Err(compact_error) => {
                        let d = detector_only_diagnosis(
                            candidates,
                            &format!(
                                "Fugu diagnosis failed ({e}); compact retry failed ({compact_error}); showing deterministic detector-only diagnosis."
                            ),
                        );
                        self.store.save_findings(&d.ranked_findings)?;
                        self.store.save_diagnosis(&d)?;
                        return Ok(d);
                    }
                }
            }
        };

        let coached = self.coach(&profile, &diagnosed).await.unwrap_or_else(|_| CoachOutput {
            ranked_holes: diagnosed.iter().map(|f| f.id.clone()).collect(),
            do_items: vec![
                "Move search/discovery into delegated subagents before reading large file sets."
                    .into(),
                "Require an explicit verification command before marking agent work complete.".into(),
            ],
            stop_items: vec![
                "Stop letting main-context searches accumulate before the first implementation edit."
                    .into(),
            ],
            narrative: "Fugu confirmed recurring workflow holes in the Claude Code transcript set."
                .into(),
        });

        let mut verified = Vec::new();
        for mut f in diagnosed.into_iter().take(6) {
            if f.severity >= 4 && !is_detector_backfill(&f) {
                match self.verify(&f).await {
                    Ok(v) if !v.refuted => {
                        f.confidence = (f.confidence * v.confidence).min(0.99);
                        f.status = "confirmed".into();
                        f.verifier_verdict = Some(v.verdict);
                        self.emit_verdict(&f, "confirmed");
                        verified.push(f);
                    }
                    Ok(v) => {
                        tracing::info!(
                            pattern = %f.pattern_id,
                            verdict = %v.verdict,
                            "finding refuted"
                        );
                        self.emit_verdict(&f, "refuted");
                    }
                    Err(e) => {
                        f.status = "confirmed".into();
                        f.verifier_verdict = Some(format!(
                            "Verifier unavailable; retained detector+diagnostician finding: {e}"
                        ));
                        self.emit_verdict(&f, "confirmed");
                        verified.push(f);
                    }
                }
            } else if is_detector_backfill(&f) {
                f.status = "detector_backfill".into();
                f.verifier_verdict = Some(
                    "Retained from deterministic detectors to keep the diagnosis at the requested top-three coverage after Fugu confirmation."
                        .into(),
                );
                self.emit_verdict(&f, "confirmed");
                verified.push(f);
            } else {
                f.status = "confirmed".into();
                self.emit_verdict(&f, "confirmed");
                verified.push(f);
            }
        }

        if verified.len() < 3 {
            verified = fill_verified_with_backfill(verified, &candidates, 3);
        }
        let ranked = rank_by_coach(verified, &coached.ranked_holes);
        let d = Diagnosis {
            id: stable_id(&[
                "diagnosis",
                &Utc::now().to_rfc3339(),
                &format!("{:?}", scope.query),
            ]),
            created_at: Utc::now(),
            ranked_findings: ranked,
            do_items: coached.do_items,
            stop_items: coached.stop_items,
            narrative: coached.narrative,
            detector_only: false,
        };
        self.store.save_findings(&d.ranked_findings)?;
        self.store.save_diagnosis(&d)?;
        Ok(d)
    }

    async fn diagnose(
        &self,
        profile: &CompetenceProfile,
        candidates: &[Finding],
    ) -> Result<Vec<Finding>> {
        let input = json!({
            "task":"Diagnose recurring holes in this operator's Claude Code agent workflow. Confirm only findings supported by evidence. Return evidence-cited JSON.",
            "profile":compact_profile(profile),
            "candidate_findings":candidates.iter().take(3).map(compact_finding).collect::<Vec<_>>()
        });
        let out: DiagnosticianOutput = self
            .call_json(
                "diagnostician",
                &brain_diagnose_model(),
                &brain_effort(true),
                serde_json::to_value(schema_for!(DiagnosticianOutput))?,
                &input,
            )
            .await?;

        let mut result = Vec::new();
        for ff in out.findings {
            if let Some(base) = candidates.iter().find(|c| c.pattern_id == ff.pattern_id) {
                let mut f = base.clone();
                f.severity = ff.severity.clamp(1, 5);
                f.frequency = ff.frequency.clamp(0.0, 1.0);
                f.est_cost_tokens = ff.est_cost_tokens;
                f.est_cost_minutes = ff.est_cost_minutes;
                f.confidence = ff.confidence.clamp(0.0, 1.0);
                f.rationale = ff.rationale;
                if !ff.evidence.is_empty() {
                    f.evidence = ff
                        .evidence
                        .into_iter()
                        .map(|e| EvidenceRef {
                            session_id: e.session,
                            turn_id: e.turn,
                            event_id: e.event,
                            quote: e.quote.map(|q| excerpt(&q, 220)),
                            source_path: None,
                        })
                        .collect();
                }
                result.push(f);
            }
        }
        Ok(fill_verified_with_backfill(result, candidates, 3))
    }

    async fn diagnose_compact(
        &self,
        profile: &CompetenceProfile,
        candidates: &[Finding],
    ) -> Result<Vec<Finding>> {
        let input = json!({
            "task":"Choose exactly the top 3 recurring Claude Code workflow holes from deterministic candidates. Use only supplied metrics/evidence. Return evidence-cited JSON with three findings.",
            "profile":compact_profile(profile),
            "candidate_findings":candidates.iter().take(5).map(compact_finding).collect::<Vec<_>>()
        });
        let out: DiagnosticianOutput = self
            .call_json(
                "diagnostician_compact",
                &brain_diagnose_model(),
                &brain_effort(false),
                serde_json::to_value(schema_for!(DiagnosticianOutput))?,
                &input,
            )
            .await?;

        let mut result = Vec::new();
        for ff in out.findings {
            if let Some(base) = candidates.iter().find(|c| c.pattern_id == ff.pattern_id) {
                let mut f = base.clone();
                f.severity = ff.severity.clamp(1, 5);
                f.frequency = ff.frequency.clamp(0.0, 1.0);
                f.est_cost_tokens = ff.est_cost_tokens;
                f.est_cost_minutes = ff.est_cost_minutes;
                f.confidence = ff.confidence.clamp(0.0, 1.0);
                f.rationale = ff.rationale;
                if !ff.evidence.is_empty() {
                    f.evidence = ff
                        .evidence
                        .into_iter()
                        .map(|e| EvidenceRef {
                            session_id: e.session,
                            turn_id: e.turn,
                            event_id: e.event,
                            quote: e.quote.map(|q| excerpt(&q, 220)),
                            source_path: None,
                        })
                        .collect();
                }
                result.push(f);
            }
        }
        Ok(fill_verified_with_backfill(result, candidates, 3))
    }

    async fn coach(
        &self,
        profile: &CompetenceProfile,
        findings: &[Finding],
    ) -> Result<CoachOutput> {
        let input = json!({
            "task":"Turn verified workflow holes into terse, practical coaching for a founder using coding agents. Be direct. Do not invent fixes beyond evidence.",
            "profile":compact_profile(profile),
            "findings":findings.iter().take(5).map(compact_finding).collect::<Vec<_>>()
        });
        self.call_json(
            "coach",
            &brain_diagnose_model(),
            &brain_effort(true),
            serde_json::to_value(schema_for!(CoachOutput))?,
            &input,
        )
        .await
    }

    async fn verify(&self, finding: &Finding) -> Result<VerifyOutput> {
        let input = json!({
            "task":"Adversarially try to refute this workflow diagnosis. If evidence is weak or alternative explanations dominate, set refuted=true. Otherwise explain why it survives.",
            "finding":compact_finding(finding)
        });
        self.call_json(
            "verifier",
            &brain_verify_model(),
            &brain_effort(false),
            serde_json::to_value(schema_for!(VerifyOutput))?,
            &input,
        )
        .await
    }

    async fn call_json<T: for<'de> Deserialize<'de>>(
        &self,
        stage: &str,
        model: &str,
        effort: &str,
        schema: Value,
        input: &Value,
    ) -> Result<T> {
        let prompt = serde_json::to_string(input)?;
        let req_hash = hex::encode(hash64(prompt.as_bytes()).to_be_bytes());

        let started = Instant::now();
        let (out, usage) = if self.app.is_none()
            || std::env::var("WARDEN_FUGU_TRANSPORT").as_deref() == Ok("curl")
        {
            self.call_json_with_curl(stage, model, effort, schema, &prompt)
                .await?
        } else {
            match self
                .call_json_streaming(stage, model, effort, schema.clone(), &prompt)
                .await
            {
                Ok(ok) => ok,
                Err(stream_error) => {
                    tracing::warn!(
                        stage,
                        error = %stream_error,
                        "streaming Fugu response failed; retrying non-streaming"
                    );
                    match self
                        .call_json_blocking(stage, model, effort, schema.clone(), &prompt)
                        .await
                    {
                        Ok(ok) => ok,
                        Err(blocking_error) => {
                            tracing::warn!(
                                stage,
                                error = %blocking_error,
                                "reqwest non-streaming Fugu response failed; retrying with curl transport"
                            );
                            match self
                                .call_json_with_curl(stage, model, effort, schema, &prompt)
                                .await
                            {
                                Ok(ok) => ok,
                                Err(curl_error) => {
                                    return Err(anyhow!(
                                        "streaming Fugu {stage} failed ({stream_error}); reqwest non-streaming retry failed ({blocking_error}); curl retry failed ({curl_error})"
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        };

        if out.trim().is_empty() {
            return Err(anyhow!("Fugu {stage} returned empty output"));
        }

        let parsed_text = repair_json_text(&out);
        let val: Value = serde_json::from_str(&parsed_text)
            .with_context(|| format!("parse Fugu {stage} JSON: {parsed_text}"))?;
        let u = usage_tokens(&usage);
        self.emit_usage(stage, &u);
        self.store.save_fugu_run(
            stage,
            model,
            effort,
            &req_hash,
            u.0,
            u.1,
            u.2,
            u.3,
            started.elapsed().as_millis() as u64,
        )?;
        Ok(serde_json::from_value(val)?)
    }

    async fn call_json_streaming(
        &self,
        stage: &str,
        model: &str,
        effort: &str,
        schema: Value,
        prompt: &str,
    ) -> Result<(String, Value)> {
        let body = fugu_body(stage, model, effort, schema, prompt, true);
        let resp = self.send_fugu(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Fugu {stage} HTTP {status}: {}",
                text.chars().take(800).collect::<String>()
            ));
        }

        let mut stream = resp.bytes_stream();
        let mut buf = String::new();
        let mut out = String::new();
        let mut usage = json!({});
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(pos) = buf.find("\n\n") {
                let frame = buf[..pos].to_string();
                buf = buf[pos + 2..].to_string();
                for line in frame.lines() {
                    let Some(data) = line.strip_prefix("data: ") else {
                        continue;
                    };
                    if data.trim() == "[DONE]" {
                        continue;
                    }
                    let Ok(v) = serde_json::from_str::<Value>(data) else {
                        continue;
                    };
                    match v.get("type").and_then(Value::as_str).unwrap_or("") {
                        "response.output_text.delta" => {
                            if let Some(delta) = v.get("delta").and_then(Value::as_str) {
                                out.push_str(delta);
                                self.emit_delta(stage, delta);
                            }
                        }
                        "response.output_text.done" => {
                            if out.trim().is_empty() {
                                if let Some(text) = v.get("text").and_then(Value::as_str) {
                                    out.push_str(text);
                                }
                            }
                        }
                        "response.completed" => {
                            if let Some(u) = v.get("response").and_then(|r| r.get("usage")) {
                                usage = u.clone();
                            }
                            if out.trim().is_empty() {
                                out =
                                    extract_output_text(v.get("response").unwrap_or(&Value::Null));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Ok((out, usage))
    }

    async fn call_json_blocking(
        &self,
        stage: &str,
        model: &str,
        effort: &str,
        schema: Value,
        prompt: &str,
    ) -> Result<(String, Value)> {
        let body = fugu_body(stage, model, effort, schema, prompt, false);
        let resp = self.send_fugu(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "Fugu {stage} HTTP {status}: {}",
                text.chars().take(800).collect::<String>()
            ));
        }
        let response: Value = resp.json().await?;
        let usage = response.get("usage").cloned().unwrap_or_else(|| json!({}));
        Ok((extract_output_text(&response), usage))
    }

    async fn call_json_with_curl(
        &self,
        stage: &str,
        model: &str,
        effort: &str,
        schema: Value,
        prompt: &str,
    ) -> Result<(String, Value)> {
        let key = self.api_key.as_ref().context("SAKANA_API_KEY missing")?;
        let body = fugu_body(stage, model, effort, schema, prompt, false);
        let mut child = tokio::process::Command::new("curl")
            .arg("--silent")
            .arg("--show-error")
            .arg("--fail-with-body")
            .arg("--max-time")
            .arg(fugu_curl_timeout_secs().to_string())
            .arg("--connect-timeout")
            .arg("10")
            .arg("--header")
            .arg(format!("Authorization: Bearer {key}"))
            .arg("--header")
            .arg("Content-Type: application/json")
            .arg("--header")
            .arg("Accept: application/json")
            .arg("--header")
            .arg("Accept-Encoding: identity")
            .arg("--data-binary")
            .arg("@-")
            .arg(brain_responses_url())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("spawn curl for Fugu retry")?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(serde_json::to_string(&body)?.as_bytes())
                .await?;
        }
        let output = child.wait_with_output().await?;
        if !output.status.success() {
            return Err(anyhow!(
                "curl Fugu {stage} exited {}: {}{}",
                output.status,
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
                    .chars()
                    .take(800)
                    .collect::<String>()
            ));
        }
        let response: Value = serde_json::from_slice(&output.stdout)?;
        let usage = response.get("usage").cloned().unwrap_or_else(|| json!({}));
        Ok((extract_output_text(&response), usage))
    }

    fn send_fugu(&self, body: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(brain_responses_url())
            .bearer_auth(self.api_key.as_ref().expect("checked before Fugu call"))
            .header(header::ACCEPT, "application/json")
            // The Responses SSE endpoint has intermittently returned compressed chunks that
            // reqwest surfaced as `error decoding response body` in the app. Identity encoding
            // keeps the war-room stream simple and deterministic, and the non-stream retry below
            // makes the diagnosis path resilient if SSE is unavailable.
            .header(header::ACCEPT_ENCODING, "identity")
            .json(body)
    }

    fn emit_delta(&self, stage: &str, delta: &str) {
        if let Some(app) = &self.app {
            let _ = app.emit("fugu_delta", json!({"stage":stage,"delta":delta}));
        }
    }

    fn emit_usage(&self, stage: &str, u: &(u64, u64, u64, u64)) {
        if let Some(app) = &self.app {
            let _ = app.emit(
                "fugu_usage",
                json!({
                    "stage":stage,
                    "input_tokens":u.0,
                    "output_tokens":u.1,
                    "orchestration_input_tokens":u.2,
                    "orchestration_output_tokens":u.3
                }),
            );
        }
    }
}

fn fugu_body(
    stage: &str,
    model: &str,
    effort: &str,
    schema: Value,
    prompt: &str,
    stream: bool,
) -> Value {
    let mut body = json!({
        "model":model,
        "input":[
            {"role":"system","content":[{"type":"input_text","text":"You are WARDEN's analysis brain. Return only valid JSON matching the schema. Cite only supplied evidence."}]},
            {"role":"user","content":[{"type":"input_text","text":prompt}]}
        ],
        "text":{"format":{"type":"json_schema","name":stage,"schema":schema,"strict":false}},
        "reasoning":{"effort":effort},
        "stream":stream,
        "max_output_tokens":fugu_max_output_tokens(stage)
    });
    if stream {
        body["stream_options"] = json!({"include_usage":true});
    }
    body
}

fn fugu_curl_timeout_secs() -> u64 {
    // Keep the app-facing reqwest timeout short so the UI never hangs on a bad network call,
    // but give the final curl fallback enough room for high-effort Fugu war-room runs.
    std::env::var("WARDEN_FUGU_CURL_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| {
            std::env::var("WARDEN_FUGU_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map(|s| (s * 3).clamp(60, 240))
        })
        .unwrap_or(150)
}

fn fugu_max_output_tokens(stage: &str) -> u64 {
    match stage {
        "diagnostician" => 3_000,
        "coach" => 1_500,
        "verifier" => 900,
        _ => 2_000,
    }
}

fn compact_profile(p: &CompetenceProfile) -> Value {
    json!({
        "session_count":p.session_count,
        "event_count":p.event_count,
        "finding_count":p.finding_count,
        "token_burn_total":p.token_burn_total,
        "avg_prompt_specificity":p.avg_prompt_specificity,
        "avg_cache_read_ratio":p.avg_cache_read_ratio,
        "avg_tool_error_rate":p.avg_tool_error_rate,
        "no_delegation_sessions":p.no_delegation_sessions,
        "context_bloat_sessions":p.context_bloat_sessions,
        "unverified_sessions":p.unverified_sessions,
        "repeated_explanation_clusters":p.repeated_explanation_clusters.iter().take(3).map(|c|json!({
            "phrase":c.phrase,
            "count":c.count,
            "session_ids":c.session_ids.iter().take(3).collect::<Vec<_>>()
        })).collect::<Vec<_>>()
    })
}

fn compact_finding(f: &Finding) -> Value {
    json!({
        "id":f.id,
        "pattern_id":f.pattern_id,
        "title":f.title,
        "severity":f.severity,
        "frequency":f.frequency,
        "est_cost_tokens":f.est_cost_tokens,
        "est_cost_minutes":f.est_cost_minutes,
        "confidence":f.confidence,
        "rationale":f.rationale,
        "evidence":f.evidence.iter().take(1).map(|e|json!({
            "session_id":e.session_id,
            "turn_id":e.turn_id,
            "event_id":e.event_id,
            "quote":e.quote.as_ref().map(|q|excerpt(q,120)),
            "source_path":e.source_path
        })).collect::<Vec<_>>()
    })
}

fn detector_only_diagnosis(findings: Vec<Finding>, narrative: &str) -> Diagnosis {
    Diagnosis {
        id: stable_id(&["detector-only", &Utc::now().to_rfc3339()]),
        created_at: Utc::now(),
        ranked_findings: findings,
        do_items: vec![
            "Delegate broad search/reading into subagents before main-context implementation."
                .into(),
            "Add an explicit test/build verification requirement to agent prompts.".into(),
            "Move repeated project context into durable CLAUDE.md guidance.".into(),
        ],
        stop_items: vec![
            "Stop accepting 'done' without a recorded verification command.".into(),
            "Stop spending main context on exploratory file inventory.".into(),
        ],
        narrative: narrative.into(),
        detector_only: true,
    }
}

fn rank_by_coach(mut findings: Vec<Finding>, ranked: &[String]) -> Vec<Finding> {
    findings.sort_by_key(|f| {
        ranked
            .iter()
            .position(|id| id == &f.id || id == &f.pattern_id)
            .unwrap_or(999)
    });
    findings.truncate(3);
    findings
}

fn fill_verified_with_backfill(
    confirmed: Vec<Finding>,
    candidates: &[Finding],
    minimum: usize,
) -> Vec<Finding> {
    let original_len = confirmed.len();
    fill_missing_findings(confirmed, candidates, minimum)
        .into_iter()
        .enumerate()
        .map(|(idx, f)| {
            if idx < original_len {
                f
            } else {
                mark_detector_backfill(f)
            }
        })
        .collect()
}

fn fill_missing_findings(
    mut confirmed: Vec<Finding>,
    candidates: &[Finding],
    minimum: usize,
) -> Vec<Finding> {
    for candidate in candidates {
        if confirmed.len() >= minimum {
            break;
        }
        if confirmed
            .iter()
            .any(|f| f.id == candidate.id || f.pattern_id == candidate.pattern_id)
        {
            continue;
        }
        confirmed.push(candidate.clone());
    }
    if confirmed.is_empty() {
        candidates.to_vec()
    } else {
        confirmed
    }
}

fn mark_detector_backfill(mut f: Finding) -> Finding {
    f.status = "detector_backfill".into();
    f.verifier_verdict = Some(
        "Retained from deterministic detectors to keep the diagnosis at the requested top-three coverage after Fugu confirmation."
            .into(),
    );
    f
}

fn is_detector_backfill(f: &Finding) -> bool {
    f.status == "detector_backfill"
}

fn extract_output_text(response: &Value) -> String {
    if let Some(t) = response.get("output_text").and_then(Value::as_str) {
        return t.to_string();
    }

    let mut s = String::new();
    if let Some(arr) = response.get("output").and_then(Value::as_array) {
        for item in arr {
            if let Some(ca) = item.get("content").and_then(Value::as_array) {
                for c in ca {
                    if c.get("type").and_then(Value::as_str) == Some("output_text") {
                        if let Some(t) = c.get("text").and_then(Value::as_str) {
                            s.push_str(t);
                        }
                    }
                }
            }
        }
    }
    s
}

fn repair_json_text(s: &str) -> String {
    let mut t = s.trim();
    if let Some(rest) = t.strip_prefix("```json") {
        t = rest.trim_start_matches(['\n', '\r']).trim();
    } else if let Some(rest) = t.strip_prefix("```") {
        t = rest.trim_start_matches(['\n', '\r']).trim();
    }
    if let Some(rest) = t.strip_suffix("```") {
        t = rest.trim();
    }
    if t.starts_with('{') || t.starts_with('[') {
        t.to_string()
    } else if let Some(i) = t.find('{') {
        t[i..].to_string()
    } else if let Some(i) = t.find('[') {
        t[i..].to_string()
    } else {
        t.to_string()
    }
}

/// Resolve the harness for a finding from the session its first evidence cites.
/// `Finding` carries no harness field, so we look up the harness of
/// `finding.evidence[0].session_id` in the store. No evidence, an unknown
/// session, or a store error all degrade to `"unknown"` — a verdict is never
/// dropped just because its harness can't be resolved.
fn resolve_harness(store: &Store, finding: &Finding) -> String {
    finding
        .evidence
        .first()
        .and_then(|e| store.session_harness(&e.session_id).ok().flatten())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Build the `candidates_nominated` event payload from the deterministic
/// candidate findings. Pure + store-only so the event contract is unit-testable
/// without a Tauri runtime; `emit_candidates` just forwards this to `app.emit`.
/// Shape (locked, consumed by the war-room):
/// `{ "candidates": [ { "pattern_id", "session_id", "harness", "severity_hint" } ] }`.
/// `harness` is the snake_case `Harness::as_str()` value resolved per candidate.
fn candidates_payload(candidates: &[Finding], store: &Store) -> Value {
    let items = candidates
        .iter()
        .map(|f| {
            let session_id = f
                .evidence
                .first()
                .map(|e| e.session_id.clone())
                .unwrap_or_default();
            json!({
                "pattern_id": f.pattern_id,
                "session_id": session_id,
                "harness": resolve_harness(store, f),
                "severity_hint": f.severity,
            })
        })
        .collect::<Vec<_>>();
    json!({ "candidates": items })
}

/// Build one `finding_verdict` event payload. Pure + store-only for the same
/// reason as `candidates_payload`. Shape (locked):
/// `{ "finding_id", "pattern_id", "harness", "verdict":"confirmed"|"refuted", "severity" }`.
fn verdict_payload(finding: &Finding, verdict: &str, store: &Store) -> Value {
    json!({
        "finding_id": finding.id,
        "pattern_id": finding.pattern_id,
        "harness": resolve_harness(store, finding),
        "verdict": verdict,
        "severity": finding.severity,
    })
}

impl Brain {
    /// Emit `candidates_nominated` once the deterministic candidates exist and
    /// before any Fugu call. No-op when headless (`app` is None, e.g. tests).
    fn emit_candidates(&self, candidates: &[Finding]) {
        if let Some(app) = &self.app {
            let _ = app.emit("candidates_nominated", candidates_payload(candidates, &self.store));
        }
    }

    /// Emit one `finding_verdict` during/after the Verifier. `confirmed` =
    /// survived (or verifier unavailable but retained); `refuted` = dropped.
    /// No-op when headless.
    fn emit_verdict(&self, finding: &Finding, verdict: &str) {
        if let Some(app) = &self.app {
            let _ = app.emit("finding_verdict", verdict_payload(finding, verdict, &self.store));
        }
    }
}

fn usage_tokens(u: &Value) -> (u64, u64, u64, u64) {
    (
        u.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
        u.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
        u.pointer("/input_tokens_details/orchestration_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        u.pointer("/output_tokens_details/orchestration_output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_split_output() {
        let v = json!({"output":[{"type":"message","content":[{"type":"output_text","text":"{\"ok\":"}]},{"type":"message","content":[{"type":"output_text","text":"true}"}]}]});
        assert_eq!(extract_output_text(&v), "{\"ok\":true}");
    }

    #[test]
    fn extracts_top_level_output_text() {
        let v = json!({"output_text":"{\"ok\":true}","usage":{"input_tokens":1}});
        assert_eq!(extract_output_text(&v), "{\"ok\":true}");
    }

    #[test]
    fn repairs_json_code_fence() {
        assert_eq!(
            repair_json_text("```json\n{\"ok\":true}\n```"),
            "{\"ok\":true}"
        );
    }

    #[test]
    fn fills_partial_fugu_output_to_top_three() {
        fn finding(id: &str) -> Finding {
            Finding {
                id: id.into(),
                pattern_id: id.into(),
                title: id.into(),
                severity: 4,
                frequency: 0.5,
                est_cost_tokens: 1,
                est_cost_minutes: 1,
                confidence: 0.7,
                rationale: id.into(),
                evidence: vec![],
                status: "candidate".into(),
                verifier_verdict: None,
            }
        }

        let candidates = vec![finding("a"), finding("b"), finding("c"), finding("d")];
        let filled = fill_missing_findings(vec![finding("b")], &candidates, 3);

        assert_eq!(
            filled
                .iter()
                .map(|f| f.pattern_id.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a", "c"]
        );
    }

    // ── Event contract: candidates_nominated / finding_verdict payload shapes ──
    // These lock the exact keys + snake_case harness the Task 7 war-room consumes.
    // `app.emit` only forwards these values, so testing the pure builders gives
    // the contract real coverage without a Tauri runtime.

    fn finding_with_evidence(pattern: &str, session_id: &str, severity: u8) -> Finding {
        Finding {
            id: format!("fid-{pattern}"),
            pattern_id: pattern.into(),
            title: pattern.into(),
            severity,
            frequency: 0.5,
            est_cost_tokens: 1,
            est_cost_minutes: 1,
            confidence: 0.7,
            rationale: "r".into(),
            evidence: vec![EvidenceRef {
                session_id: session_id.into(),
                turn_id: None,
                event_id: None,
                quote: None,
                source_path: None,
            }],
            status: "candidate".into(),
            verifier_verdict: None,
        }
    }

    /// Seed one session of the given harness so harness resolution has a row.
    fn seed_session(store: &Store, id: &str, harness: Harness) {
        let now = Utc::now();
        let session = Session {
            id: id.into(),
            harness,
            external_id: id.into(),
            project: None,
            model_ids: vec![],
            started_at: now,
            ended_at: None,
            source_path: std::path::PathBuf::from(format!("/tmp/{id}.jsonl")),
            raw_hash: 0,
            ingested_at: now,
            meta: json!({}),
        };
        let turn = Turn {
            id: format!("{id}-t"),
            session_id: id.into(),
            parent_id: None,
            role: Role::User,
            index: 0,
            started_at: now,
            duration_ms: None,
            is_sidechain: false,
        };
        store.upsert_session_batch(&session, &[turn], &[], 0).unwrap();
    }

    #[test]
    fn candidates_payload_has_locked_shape_with_snake_case_harness() {
        let store = Store::memory().unwrap();
        seed_session(&store, "c1", Harness::ClaudeCode);
        seed_session(&store, "x1", Harness::Codex);
        let candidates = vec![
            finding_with_evidence("CONTEXT_BLOAT", "c1", 4),
            finding_with_evidence("UNVERIFIED_COMPLETION", "x1", 5),
        ];

        let payload = candidates_payload(&candidates, &store);
        let arr = payload["candidates"].as_array().expect("candidates array");
        assert_eq!(arr.len(), 2);

        // Exactly the four locked keys, in the locked snake_case harness form.
        assert_eq!(arr[0]["pattern_id"], "CONTEXT_BLOAT");
        assert_eq!(arr[0]["session_id"], "c1");
        assert_eq!(arr[0]["harness"], "claude_code");
        assert_eq!(arr[0]["severity_hint"], 4);
        assert_eq!(arr[1]["harness"], "codex");
        assert_eq!(arr[1]["severity_hint"], 5);
    }

    #[test]
    fn candidates_payload_no_evidence_defaults_harness_unknown() {
        let store = Store::memory().unwrap();
        let mut f = finding_with_evidence("WHACK_A_MOLE", "ignored", 4);
        f.evidence.clear();
        let payload = candidates_payload(&[f], &store);
        let item = &payload["candidates"][0];
        assert_eq!(item["session_id"], "");
        assert_eq!(item["harness"], "unknown");
    }

    #[test]
    fn verdict_payload_has_locked_shape() {
        let store = Store::memory().unwrap();
        seed_session(&store, "c1", Harness::ClaudeCode);
        let f = finding_with_evidence("CONTEXT_BLOAT", "c1", 4);

        let confirmed = verdict_payload(&f, "confirmed", &store);
        assert_eq!(confirmed["finding_id"], "fid-CONTEXT_BLOAT");
        assert_eq!(confirmed["pattern_id"], "CONTEXT_BLOAT");
        assert_eq!(confirmed["harness"], "claude_code");
        assert_eq!(confirmed["verdict"], "confirmed");
        assert_eq!(confirmed["severity"], 4);

        let refuted = verdict_payload(&f, "refuted", &store);
        assert_eq!(refuted["verdict"], "refuted");
    }
}
