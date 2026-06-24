# Near AI Engine Swap — OpenAI-compatible Brain

- **Date:** 2026-06-23
- **Status:** Approved (brainstorm) → implementing
- **Scope:** Milestone-adjacent **infrastructure**, not an M3 RADAR feature. Replaces the reasoning
  engine's wire layer; the diagnose→coach→verify pipeline, detectors, events, and viz contract are
  preserved.

## 1. Motivation

WARDEN's brain currently talks to Sakana **Fugu** over OpenAI's newer **Responses API**
(`POST /v1/responses`, `input[]`, `text.format.json_schema`, `reasoning.effort`, SSE
`response.output_text.delta`). We are switching the engine to **Near AI cloud** (GLM-4.5 / 4.6
family), which is consumed as a standard OpenAI client. Near AI exposes the older, far more widely
implemented **Chat Completions** dialect (`POST /v1/chat/completions`, `messages[]`,
`response_format`, `choices[].delta.content`, `usage.prompt_tokens`).

So "OpenAI-compatible" means a **real wire-format adapter**, not a base-URL swap. The env-swap
scaffolding (`WARDEN_BRAIN_*`) already exists; we teach the brain the Chat Completions dialect and
retire the Responses dialect.

## 2. Goals / Non-goals

**Goals**
1. `brain.rs` speaks **only** OpenAI Chat Completions. Fugu Responses wire layer is removed.
2. A clean, provider-agnostic **ENV var framework** (`WARDEN_BRAIN_*`), filled in later.
3. Secrets/config read from a **project-local `.env`** (already gitignored) loaded at startup via
   `dotenvy` — works identically under `pnpm tauri dev`, Finder, or a login item.
4. Structured-output strategy that **degrades gracefully** across providers.
5. **Honest viz**: Chat Completions has no orchestration tokens → emit honest zeros; the war-room
   degrades to delta pulses + plain weight, never faking orchestration flares.
6. New unit tests cover the Chat Completions request shape, SSE parsing, and usage mapping.

**Non-goals**
- M3 RADAR feature work (separate spec).
- GLM "thinking"/reasoning-effort knob (future; Chat Completions has no standard `reasoning` field).
- Apply / voice / fleet / screen (M4+; remain `not_in_slice` stubs).
- Production `.app` packaging concerns beyond the `.env` loader (deferred).

## 3. Decisions (locked in brainstorm)

| # | Decision |
|---|----------|
| D1 | **Replace Fugu entirely** — do not keep a dual-protocol switch. |
| D2 | Wire dialect = OpenAI **Chat Completions**. |
| D3 | Secrets/config live in a **project-local `.env`** (gitignored); `~/.zshrc` is not involved. |
| D4 | Canonical env names are `WARDEN_BRAIN_*`, with `OPENAI_*` accepted as fallback for key/base. |
| D5 | Keep the `fugu_delta` / `fugu_usage` **event names** (frontend listens to them) to avoid UI churn; only the wire layer behind them changes. |
| D6 | Rename backend env `WARDEN_FUGU_*` → `WARDEN_BRAIN_*`; drop the `SAKANA_API_KEY` fallback. |

## 4. Architecture

### 4.1 Request (Chat Completions)
```
POST {base_url}/chat/completions          # base_url already ends in /v1
Authorization: Bearer {key}
Content-Type: application/json
Accept: application/json
Accept-Encoding: identity                  # keep: avoids SSE chunk re-decode issues

{
  "model": "<model>",
  "messages": [
    {"role": "system", "content": "<system prompt>"},
    {"role": "user",   "content": "<stage prompt JSON>"}
  ],
  "stream": <bool>,
  "max_tokens": <int>,                       # 3000 diag / 1500 coach / 900 verify
  "response_format": <see 4.4>,              # omitted in `prompt` mode
  "stream_options": {"include_usage": true}  # only when stream=true
}
```
`base_url` has its trailing `/` stripped, then `/chat/completions` appended (mirrors the old
`brain_responses_url` helper, new path).

### 4.2 Response — blocking
```
{ "choices": [ { "message": { "content": "<text>" } } ],
  "usage": { "prompt_tokens": N, "completion_tokens": M, "total_tokens": T } }
```
Extract `choices[0].message.content`.

### 4.3 Response — streaming (SSE)
Frames split on `\n\n`; each `data:` line is a JSON chunk or `[DONE]`.
- Content delta: `choices[0].delta.content` (string) → accumulate + `emit_delta(stage, delta)`.
- Usage: the final chunk (with `stream_options.include_usage`) carries `usage` (often with empty
  `choices`). Parse `usage` from **any** chunk that contains it (provider-tolerant).
- `data: [DONE]` terminates.

### 4.4 Structured output — `WARDEN_BRAIN_STRUCTURED_OUTPUT`
The pipeline requires strict JSON per stage. Three modes, all backed by the existing
`repairs_json_text` (code-fence strip) + serde parse as the safety net:

| Mode | `response_format` sent |
|------|------------------------|
| `json_schema` | `{type:"json_schema", json_schema:{name:<stage>, schema:<schema>, strict:true}}` |
| `json_object` (**default**) | `{type:"json_object"}` — schema is already embedded in the stage prompt |
| `prompt` | *(field omitted)* — rely on prompt + JSON repair only |

Default is `json_object`: broadly supported by GLM and OpenAI-compatible hosts. Operators opt into
strict `json_schema` when the provider supports it.

### 4.5 Usage mapping (honest viz)
`usage_tokens` maps Chat Completions → the existing 4-tuple:
```
input_tokens                 = usage.prompt_tokens
output_tokens                = usage.completion_tokens
orchestration_input_tokens   = 0      # Chat Completions has no orchestration phase
orchestration_output_tokens  = 0
```
`emit_usage` emits the tuple unchanged. With orchestration = 0 the war-room must render the degraded
state (delta pulses + plain node weight) and must not synthesize orchestration flares. We **verify**
`src/viz/bridge.ts` handles zero honestly (fix if it fakes or divides by zero).

### 4.6 Transport & fallback
Keep the existing 3-stage dispatch (`streaming → blocking → curl`) and the headless/`curl`-transport
branch, retargeted at `/chat/completions`. The curl fallback body and headers are unchanged except
the URL.

## 5. Config & secrets

### 5.1 Loader
Add `dotenvy` to `src-tauri/Cargo.toml`. Call `dotenvy::dotenv().ok()` as the **first line** of the
Tauri entry (`lib.rs` `run()`), before `Store`/brain env reads. `dotenvy::dotenv()` walks up from the
process CWD, so a repo-root `.env` is found under `pnpm tauri dev` (CWD = `src-tauri/`). Existing
`std::env::var(...)` reads then work unchanged.

### 5.2 ENV var framework (`util.rs`)
Rename `WARDEN_FUGU_*` → `WARDEN_BRAIN_*`, drop `SAKANA_API_KEY`. Resolution = env → (`OPENAI_*`
fallback for key/base) → default.

| Var | Default | Notes |
|-----|---------|-------|
| `WARDEN_BRAIN_API_KEY` | *(none)* | fallback `OPENAI_API_KEY`. Missing key ⇒ detector-only diagnosis. |
| `WARDEN_BRAIN_BASE_URL` | *(none)* | fallback `OPENAI_BASE_URL`, then `OPENAI_API_BASE`. Missing base ⇒ detector-only. |
| `WARDEN_BRAIN_DIAGNOSE_MODEL` | `glm-4.6` | smart tier (placeholder; override in `.env`). |
| `WARDEN_BRAIN_VERIFY_MODEL` | `glm-4.5` | fast tier (placeholder; override in `.env`). |
| `WARDEN_BRAIN_STRUCTURED_OUTPUT` | `json_object` | `json_schema` \| `json_object` \| `prompt`. |
| `WARDEN_BRAIN_STREAM` | `1` | `0` to force blocking. |
| `WARDEN_BRAIN_TIMEOUT_SECS` | `75` | renamed from `WARDEN_FUGU_TIMEOUT_SECS`. |
| `WARDEN_BRAIN_CURL_TIMEOUT_SECS` | `(3×timeout).clamp(60,240)` | renamed from `WARDEN_FUGU_*`. |
| `WARDEN_BRAIN_TRANSPORT` | *(auto)* | `curl` to force the curl path; renamed from `WARDEN_FUGU_*`. |

`reasoning.effort` and `WARDEN_BRAIN_EFFORT` are removed (Responses-only). Missing key **or** base
url ⇒ the pipeline returns the detector-only diagnosis (the existing no-key branch), logged.

### 5.3 `.env.example` (committed) — rewritten from the Sakana version
```
# WARDEN brain — Near AI cloud (OpenAI-compatible Chat Completions). Fill these in your .env (gitignored).
WARDEN_BRAIN_API_KEY=
WARDEN_BRAIN_BASE_URL=          # e.g. https://api.near.ai/v1
WARDEN_BRAIN_DIAGNOSE_MODEL=    # e.g. glm-4.6
WARDEN_BRAIN_VERIFY_MODEL=      # e.g. glm-4.5
# Optional knobs
WARDEN_BRAIN_STRUCTURED_OUTPUT=json_object   # json_schema | json_object | prompt
WARDEN_BRAIN_STREAM=1
# Optional overrides
WARDEN_DB_PATH=~/.warden/warden.db
WARDEN_CLAUDE_PROJECTS=~/.claude/projects
```

## 6. What gets retired

- `brain.rs`: `fugu_body`, `send_fugu`, the `/responses` URL + `input[]`/`text.format`/`reasoning`
  blocks, and `extract_output_text`'s Responses `output[]` shape. Replaced by chat-completions
  builders/extractors.
- `util.rs`: `brain_responses_url`, `SAKANA_API_KEY`, the `WARDEN_FUGU_*` names, `brain_effort`.
- `.env.example`: the `SAKANA_API_KEY` block.
- The Responses-shape golden tests (`extracts_split_output`, `extracts_top_level_output_text`) are
  rewritten for the Chat Completions shape.

## 7. Testing

New / updated **pure unit tests** in `brain.rs` (no Tauri runtime, no network):
1. Request builder → `messages[]`, `model`, `stream`, `max_tokens`, and the correct `response_format`
   per structured-output mode (and omitted in `prompt` mode).
2. SSE parse → fed sample frames, accumulates `choices[].delta.content` and extracts `usage`.
3. Blocking parse → `choices[0].message.content` extraction.
4. Usage mapping → `prompt/completion_tokens` → `(in, out, 0, 0)`.
5. Retain `repairs_json_text`, `fills_partial_*`, and the `candidates_nominated` /
   `finding_verdict` event-contract tests unchanged.

**Verification gates** (`superpowers:verification-before-completion`):
- `cd src-tauri && cargo test` green.
- `cd src-tauri && cargo check` green.
- `pnpm build` green.
- `grep -ri 'sakana\|/responses\|WARDEN_FUGU' src-tauri/src` returns only historical comments, if any.
- Honest-viz: with orchestration = 0, the war-room shows the degraded state (manually reasoned from
  `bridge.ts`, fixed if needed).

## 8. Risks / assumptions

- **A1** Near AI is Chat Completions compatible — corroborated by the `OPENAI_API_BASE`/
  `OPENAI_API_KEY` convention; confirm against docs when available.
- **A2** Structured-output support varies by provider → made configurable, with `json_object`
  default + JSON-repair net.
- **A3** Exact model id and base URL are **placeholders**, filled by the user in `.env` later.
- **A4** `max_tokens` (not `max_completion_tokens`) — standard for GLM/Chat Completions hosts.

## 9. Definition of Done

1. `brain.rs` issues Chat Completions requests; Fugu Responses wire layer removed.
2. `dotenvy` loads the project-local `.env`; `WARDEN_BRAIN_*` framework in place; `.env.example`
   rewritten; `.env` remains gitignored.
3. Structured-output modes implemented with the documented default + fallback.
4. Honest zeros for orchestration tokens; viz degradation verified.
5. New wire-layer unit tests pass; existing event-contract/JSON-repair tests still pass.
6. `cargo test`, `cargo check`, and `pnpm build` all green with real output captured.
7. Engine runs end-to-end against an OpenAI-compatible endpoint once the user fills `.env` (user-side,
   later).
