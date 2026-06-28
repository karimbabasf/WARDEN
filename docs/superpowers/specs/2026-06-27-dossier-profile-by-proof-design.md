# DOSSIER — "Profile by Proof" — Design Spec

> **Status:** approved design (2026-06-27). Supersedes the idea capture
> `docs/ideas/2026-06-24-dossier-profile-by-proof.md` (which remains as background).
> **Branch:** `dossier` (isolated git worktree at `/Users/karimbaba/WARDEN-dossier`).
> **Milestone:** DOSSIER. Numbering deferred (M4 already = Forge); referred to by name, not number.

---

## 1. Goal

WARDEN today produces a **point-in-time diagnosis**. DOSSIER promotes that into a
**persistent, longitudinal, evidence-cited profile of the operator** — a full-page report on
*how this person drives their agents over time*: their orchestration style, signature
patterns/habits, strengths, holes, where they bleed tokens/time/rework, what kinds of projects
they build, and whether they're improving. It is anchored by a **research-grounded efficiency
score** that is defensible, reproducible, and not an LLM vibe.

**The "by proof" contract:** every claim in the dossier cites real sessions/turns. No confident
assertion survives without ≥N evidence instances in the active window. The product is
trustworthy or it is nothing.

### Non-negotiable design forces
1. **Honesty over coverage.** Confident-but-wrong is the #1 failure mode of longitudinal
   profiling. The by-proof guard (§12) and reproducible scoring (§4) exist to kill it.
2. **Cost.** A window can span thousands of sessions; raw transcripts can never enter an LLM.
   Deterministic aggregation does the heavy lifting; the LLM sees only compact aggregates +
   sampled evidence (§9).
3. **Privacy.** On-device by default. The only egress is the existing brain (NEAR AI). The new
   embeddings subsystem runs locally (§10). No new exfil surface.
4. **Isolation from Habits.** DOSSIER **reads** existing data; it **writes only its own new
   tables**. It never modifies the Habits feature, its tables, or its commands.

---

## 2. Scope

**In scope (Full DOSSIER — all 7 dimensions):**
1. Orchestration style (RADAR-sourced: concurrency, delegation, hierarchy depth, context
   discipline, harness mix).
2. Signature patterns / habits (recurring behaviors, good and bad).
3. Holes & mistakes (existing detector taxonomy, **aggregated + trended** across the window).
4. Strengths (what they consistently do well — balance, not a hit list).
5. Where they lose (token/time/rework leakage, **quantified + ranked**).
6. Project archetypes (what they build; how behavior shifts per archetype) — **needs embeddings + clustering**.
7. Trajectory (per-trait trend lines: improving / regressing / plateaued).

Plus the two concrete operator-facing surfaces:
- **Efficiency score** (hybrid, research-grounded — §4).
- **Activity heatmap** (GitHub-style, token-driven, per-harness hover — §7).

**Out of scope (this build):**
- UI polish / cinematic treatment. **Functionality first.** The Profile surface is a working
  full-page view; aesthetic pass is a later milestone.
- Scheduled/nightly profile builds. v1 is **on-demand + cached**; a background scheduler can come later.
- Any Forge "apply" integration. The natural synergy (dossier finds durable holes → Forge fixes
  them) is noted but not built here.

---

## 3. Architecture

New, self-contained backend module: **`src-tauri/src/dossier/`**. Each submodule has one purpose,
a well-defined interface, and is unit-testable in isolation.

```
src-tauri/src/dossier/
├── mod.rs          // public API: build_profile(scope), get_profile(window), orchestration
├── scope.rs        // window enum → cutoff timestamp → scoped session-id set
├── aggregate.rs    // Layer 2: deterministic rollups (time bins, per-project, per-pattern)
├── outcome.rs      // composite "good session" signal (§6) — the efficiency denominator's pair
├── efficiency.rs   // Layer 5a: deterministic sub-scores from the rubric → headline score (§4)
├── rubric.rs       // the versioned rubric: metric families, normalization, pinned weights (§4/§5)
├── summarize.rs    // Layer 3: map-reduce LLM summaries (per-session micro → week/project rollup)
├── cluster.rs      // Layer 4: on-device embeddings + sqlite-vec → archetypes + semantic dedup
├── synthesize.rs   // Layer 5b: GLM-5.2 profile synthesis → structured Profile + narrative
├── proof.rs        // Layer 6: by-proof guard (N-evidence threshold, sample-size gating)
├── heatmap.rs      // activity heatmap aggregation (token/day, per-harness, composition)
└── trajectory.rs   // per-trait trend lines from time-bucketed aggregates + profile_history
```

**Data flow (one build of one window):**

```
window (all-time/6mo/3mo/30d/2wk)
  → scope.rs:        cutoff ts → scoped session set
  → aggregate.rs:    per-session FeatureVectors + RADAR signals + outcome.rs scores
                     → time-bucketed bins, per-project rollups, per-pattern freq/cost/trend
  → summarize.rs:    per-session micro-summaries (CACHED forever by content hash)
                     → fold into per-week / per-project rollup summaries
  → cluster.rs:      embeddings over sessions/patterns → project archetypes + dedup recurring holes
  → efficiency.rs:   deterministic sub-scores (rubric.rs) → headline efficiency score
  → synthesize.rs:   GLM-5.2 over {aggregates + rollup summaries + evidence samples + sub-scores}
                     → structured Profile: per-dimension narrative + ranked leaks + trajectory
                     → LLM proposes family weights w/ rationale → pinned into rubric version
  → proof.rs:        downgrade any claim under the evidence/sample-size threshold to "emerging"
  → persist:         profiles cache (keyed by window + data_hash) → instant re-toggle
  → commands.rs:     get_profile / get_activity_heatmap / get_efficiency_score → full-page surface
```

The heatmap and the deterministic half of the score require **no LLM** and are computed
end-to-end from local data. The LLM is invoked only for summarization, weighting, and synthesis.

---

## 4. The efficiency score (hybrid — the heart)

**Decision:** hybrid = deterministic sub-scores (the grounded anchor) + LLM weighting + narration,
with the weighting **pinned and versioned** so the headline number is reproducible run-to-run.

**Structure:**
- The research (§5) produces a **rubric** of ~6–8 **metric families**. Candidate families (to be
  finalized/extended/pruned by research, not assumed):
  - **Delegation quality** — does work that should be delegated get delegated; subagent spawn
    rate vs task complexity; "search-in-main-context" anti-pattern rate.
  - **Context discipline** — context-saturation peaks, context-bloat sessions, right-sizing.
  - **Rework / thrash** — file churn, thrash index, re-prompt count (negative; lower is better).
  - **Verification rate** — verifying before claiming done; test/build runs before completion.
  - **Cache hygiene** — cache-read ratio; whether work is structured to reuse cached context.
  - **Parallelism / orchestration shape** — concurrency, hierarchy depth appropriateness.
  - **Prompt specificity** — specificity of instructions (negative correlation with re-prompting).
  - **(reserved)** — any family the research surfaces as a strong, defensible efficiency signal.
- Each family is computed **deterministically in Rust** from real per-session signals
  (`FeatureVector`, RADAR signals, outcome composite) and **normalized 0–1** via a documented
  transform (the rubric pins the normalization — e.g. percentile vs absolute threshold).
- The **headline efficiency score** = weighted combination of family sub-scores. **The initial
  (v1) weights are set by the Phase 0 research rubric (§5), not the LLM** — so `efficiency.rs`
  always has pinned weights from `rubric.rs` to compute a deterministic, reproducible headline.
  The LLM's role is strictly (a) to **propose *revised* weights with written rationale** during
  synthesis (a re-tuning suggestion for a *future* version), and (b) to **narrate** the result.
  Any proposed re-weighting is **frozen into `rubric.rs` as a new versioned constant**
  (`RUBRIC_VERSION`, e.g. `dossier-rubric-v1` → `v2`) before it is ever used — reviewed like code.
  The LLM never re-rolls the live number; a given `RUBRIC_VERSION` + signal set always yields the
  same score.
- **Auditability:** the headline score is **never shown alone**. Every sub-score, its raw inputs,
  and the active rubric version are surfaced alongside it. A skeptical operator can trace the
  number to its signals.

**Why this satisfies "not a number Claude spins up":** the number is a deterministic function of
real signals under a frozen, documented rubric. The LLM contributes the *interpretation*, not the
*arithmetic*. Two builds of the same window produce the same score.

---

## 5. The research methodology (Phase 0 — "dig and discover")

The rubric is **derived from genuine research**, two passes, before the scorer is finalized.

**Pass 1 — External frontier (multi-agent research workflow).** Spin up research agents (the
user explicitly opted into multi-agent orchestration for this) to investigate, not scrape:
- What separates expert multi-agent operators: orchestration efficiency, delegation patterns,
  context engineering, verification loops, parallelism, when subagents help vs hurt.
- Academic + industry sources on agentic-workflow efficiency, LLM token economics, multi-agent
  interoperability, prompt efficiency, context management.
- Adversarial verification of claims (the research harness verifies before accepting), so the
  rubric rests on corroborated principles, not blog hot-takes.
- **Output:** a synthesized, cited set of candidate efficiency principles + how each maps to a
  *measurable* signal we can compute from WARDEN's data.

**Pass 2 — Empirical corpus mining (local, deterministic).** Mine the operator's own transcript
corpus to ground and personalize the rubric:
- For each candidate metric from Pass 1, check: does it actually appear in this corpus? Does it
  **correlate with the outcome signal** (§6) — i.e. do high-scoring sessions on this metric
  genuinely have lower rework / fewer errors / better verification?
- Surface patterns **unique to this operator** that Pass 1 didn't predict (emergent signatures).
- Prune metrics that don't discriminate; keep metrics that separate good from bad sessions.

**Deliverable:** `docs/superpowers/research/dossier-efficiency-rubric.md` — a **versioned rubric
document**: each metric family, its precise definition, the signal(s) it reads, its normalization,
its evidence basis (external + empirical), and the initial weights. This doc is the source of
truth that `rubric.rs` implements. `RUBRIC_VERSION` ties the code to this doc.

**Gate:** Phase 1+ code (the deterministic scorer) is written **against the finalized rubric**.
The research genuinely precedes the algorithm — we do not guess metrics now.

---

## 6. The outcome signal (what "good" means)

Efficiency = useful output ÷ resources spent. Resources (tokens/time) are directly measured; the
**output/quality side** is the **composite outcome signal**, computed deterministically per session:

- **Backbone (computed signals):** low rework (thrash index, file churn, re-prompt count), low
  tool-error rate, strong verification discipline, healthy cache use, right-sized delegation —
  all already produced per-session by `featurizer.rs` / `detectors.rs`.
- **Hard-marker anchors (where present in the transcript):** code committed, tests/build run,
  errors resolved, task declared done. These are objective ground-truth boosts when the signals
  exist; their absence is not penalized (workflows vary).
- The composite is a documented function (defined alongside the rubric) producing a per-session
  **outcome score** used as the correlation target in Pass 2 and as the quality term in efficiency.

This is the denominator's partner: it stops the score from collapsing into "fewer tokens = better."

---

## 7. The activity heatmap

GitHub-contribution-graph style, fully deterministic:
- **Grid:** one cell per day across the active window.
- **Fill intensity:** total tokens that day (input+output+thinking+cached), bucketed into
  intensity levels.
- **Hover detail:** token breakdown split **by harness** (Claude vs Codex) and **by composition**
  (input / output / thinking / cached), plus session count for the day.
- **Sources:** `radar_token_cache` (per-session composition) + `features.token_burn_total`, joined
  to `sessions.started_at` (day bucket) and `sessions.harness` (split). No LLM.
- `heatmap.rs` returns a `Vec<ActivityCell>` (date, total, per-harness map, composition map,
  session_count). Pure function over scoped sessions; golden-testable.

---

## 8. Time-window toggle

A single control: **all-time · 6mo · 3mo · 30d · 2wk**. It is *only* a recency filter.
- `scope.rs` maps the window enum → cutoff timestamp → scoped session-id set (`WHERE started_at >= cutoff`).
- Extend the existing `RunScope` (`ir.rs`) with a `window: Option<Window>` (or `since: Option<DateTime>`)
  field and thread it through. Minimal new surface; reuses the existing scoping seam.
- Every claim is presented **scoped to its window** — never "you always," always "in the last 3 months."

---

## 9. Hierarchical summarization + caching (cost control)

Raw transcripts never enter the LLM. Summarize in tiers (map-reduce):
- **Per-session micro-summary** — computed once per session, **cached forever** keyed by the
  session's content hash (`sessions.raw_hash`). Reused across every window and every future
  profile. Marginal cost of a new session ≈ one small summary call.
- **Per-week / per-project rollup** — folds micro-summaries (cheap, can be deterministic or a
  small LLM reduce).
- **Window-level synthesis** — the single expensive GLM-5.2 call (§11).

Cache table `dossier_session_summaries` keyed by `(session_id, raw_hash, summarizer_version)`.
Only *new or changed* sessions are ever summarized. This tiering is what makes "all-time over 6
months" tractable.

---

## 10. Embeddings + clustering (on-device)

For project archetypes (dimension 6) and semantic dedup of recurring patterns ("is this the same
hole again?"):
- **Embeddings on-device:** `fastembed-rs` / ONNX (e.g. `bge-small`) — no new egress surface.
- **Vector search:** the `sqlite-vec` extension over the existing SQLite store.
- **Clustering math:** pure Rust (`linfa` k-means / DBSCAN) for archetype + behavior clusters.
- New tables: `dossier_embeddings` (vector per session/pattern) + vec index. Embeddings cached by
  content hash like summaries.
- Archetype classification groups sessions by `project` then assigns an archetype label
  (web app / CLI / infra / data / viz / refactor / …) from cluster + signals.

If embeddings prove heavy or low-value in practice, archetype classification can degrade to a
signal-based heuristic; the vector subsystem remains for dedup. (Decision point flagged in plan.)

---

## 11. Profile synthesis (GLM-5.2 / NEAR AI)

The one expensive call. `synthesize.rs` consumes **compact aggregates + rollup summaries +
sampled evidence + deterministic sub-scores** (never raw transcripts) and emits a **structured
Profile**:
- Per-dimension narrative (the 7 dimensions), each claim carrying its `EvidenceRef`s.
- Ranked leaks ("your single biggest leak this quarter was X, ~N tokens / ~M min").
- Proposed efficiency-family weights + rationale (→ pinned per §4).
- Reuses the existing OpenAI-compatible NEAR AI client in `brain.rs` (GLM-5.2, `z-ai/glm-5.2`).
  New stages (`summarize_session`, `synthesize_profile`) are added beside the existing
  diagnose/coach/verify; the env-swappable brain means synthesis can later point at a stronger
  model without re-plumbing if quality demands.

---

## 12. By-proof guard

`proof.rs` enforces honesty before persistence:
- **Evidence threshold:** every trait/claim must carry **≥ N `EvidenceRef` instances** within the
  window, or it is downgraded to **"emerging / tentative,"** not asserted.
- **Trend gating:** trajectory claims (§ trajectory) require a **minimum sample size per time
  bucket**; otherwise a 2-week "trend" is noise and is suppressed or labeled low-confidence.
- Reuses the existing `EvidenceRef` system and `resolve_evidence` resolution path.

---

## 13. Data model

**New IR types (`ir.rs` additions, or a `dossier`-local types module):**
- `Window` (enum: AllTime / SixMonths / ThreeMonths / ThirtyDays / TwoWeeks).
- `Profile` { window, generated_at, data_hash, rubric_version, efficiency: EfficiencyScore,
  dimensions: Vec<ProfileDimension>, ranked_leaks: Vec<Leak>, archetypes: Vec<ProjectArchetype>,
  trajectory: Vec<TraitTrend> }.
- `ProfileDimension` { key, narrative, claims: Vec<Claim> } ; `Claim` { text, confidence,
  status (asserted/emerging), evidence: Vec<EvidenceRef> }.
- `EfficiencyScore` { headline: f64, rubric_version, families: Vec<FamilyScore> } ;
  `FamilyScore` { key, sub_score: f64, weight: f64, raw_inputs: Value }.
- `Leak`, `ProjectArchetype`, `TraitTrend`, `ActivityCell`, `SessionSummary`.
- Extend `RunScope` with `window`.

**New store tables (`store.rs`) — all `dossier_*`-prefixed, never touching Habits tables:**
- `dossier_profiles` (cache, keyed by `(window, data_hash)`).
- `dossier_session_summaries` (cache, keyed by `(session_id, raw_hash, summarizer_version)`).
- `dossier_embeddings` (+ sqlite-vec index).
- Windowed read queries over existing `sessions` / `events` / `features` / `radar_token_cache`.

**Forbidden:** writes to `habit_resolutions`, `sessions`, `turns`, `events`, `features`,
`profile`, `diagnoses`, `findings`; calls to `set_habits_window`; edits to `featurizer.rs` /
`detectors.rs` signatures that Habits depends on (additive-only if shared helpers are needed).

---

## 14. Commands + events

New `#[tauri::command]`s in `commands.rs` (distinct from the M3–M7 stubs):
- `get_profile(window) -> Profile` — returns cached profile if `(window, data_hash)` is fresh,
  else signals "needs build."
- `build_profile(window)` — runs the pipeline; emits progress events
  (`dossier_progress` with stage: scope/aggregate/summarize/cluster/score/synthesize/guard/persist).
- `get_activity_heatmap(window) -> Vec<ActivityCell>` — cheap, deterministic, no build needed.
- `get_efficiency_score(window) -> EfficiencyScore` — deterministic half is instant; full headline
  available post-build.

Events Rust→web via `app.emit` per existing convention. Frontend invokes via `invoke`.

---

## 15. Frontend surface (functionality first)

A new **full-page Profile** view that occupies the entire window when opened (per the user's
requirement). Functionality-first: render real data correctly; minimal styling.
- New surface in `src/viz/` (the Profile becomes a third instrument alongside the Habits/Radar
  switcher per the FACE visual pass), OR a dedicated full-page route — chosen at implementation
  against the current `main.ts` router + `bridge.ts`.
- Renders: efficiency score (headline + sub-score breakdown + rubric version), activity heatmap
  (with per-harness hover), the 7-dimension report, ranked leaks, trajectory, window toggle.
- Wiring through `bridge.ts`; listens for `dossier_progress`, invokes the new commands.
- The cinematic/aesthetic pass is explicitly deferred.

---

## 16. Degradation & error handling

- **No-API / brain unavailable:** detector-only mode — the deterministic half (heatmap, sub-scores,
  aggregates, by-proof-gated detector findings) renders fully; LLM narrative/weighting degrades to
  a templated readout using the last pinned weights. Mirrors the existing diagnosis fallback.
- **Insufficient data:** if a window has too few sessions to support a dimension, that dimension
  reports "insufficient data," never fabricates. The by-proof guard enforces this.
- **Cache invalidation:** `data_hash` over the scoped session set; any new/changed session in the
  window invalidates that window's cached profile (but not the per-session summary cache).

---

## 17. Testing strategy

- **Deterministic units (golden tests):** `scope`, `aggregate`, `outcome`, `efficiency`,
  `heatmap`, `trajectory` — pure functions over fixture session sets with asserted outputs.
  Same fixtures + same rubric_version → identical scores (proves reproducibility).
- **By-proof guard tests:** claims below threshold get downgraded; trends below sample-size get
  suppressed.
- **Cache tests:** second build of an unchanged window is a pure cache hit (no LLM calls); adding
  one session re-summarizes exactly one session.
- **Pipeline integration:** fixture corpus → `build_profile` → structural assertions on the
  `Profile` (dimensions present, evidence attached, no orphan claims).
- **Isolation test:** assert no DOSSIER code path writes to Habits/core tables (guard against
  regression).
- Frontend: unit-test the heatmap aggregation→render mapping and the score breakdown rendering.
- Gate: `cd src-tauri && cargo test` + `pnpm build` green before any "done" claim.

---

## 18. Phasing (the implementation plan will detail each)

- **Phase 0 — Research** → finalized versioned rubric doc (§5). *No app code.*
- **Phase 1 — Scope + deterministic aggregation + outcome composite + activity heatmap.**
  Real numbers end-to-end, no LLM. (`scope.rs`, `aggregate.rs`, `outcome.rs`, `heatmap.rs`,
  window plumbing, `get_activity_heatmap`.)
- **Phase 2 — Efficiency scoring engine** (`rubric.rs`, `efficiency.rs`, pinned weights,
  `get_efficiency_score`, golden reproducibility tests).
- **Phase 3 — Hierarchical summarization + caching** (`summarize.rs`, summary cache table).
- **Phase 4 — Embeddings + clustering + archetypes** (`cluster.rs`, fastembed + sqlite-vec).
- **Phase 5 — Profile synthesis + by-proof guard + trajectory** (`synthesize.rs`, `proof.rs`,
  `trajectory.rs`, brain stages).
- **Phase 6 — Persistence + commands + events** (`dossier_profiles`, `build_profile`/`get_profile`,
  progress events, degradation mode).
- **Phase 7 — Full-page Profile surface** (functionality-first).

Each phase ends green (`cargo test` + `pnpm build`) and is independently reviewable.

---

## 19. Privacy

On-device by default. The only network egress is the existing brain (NEAR AI), and only ever the
compact aggregates + summaries + evidence samples — never raw transcripts. Embeddings are computed
locally. No new exfiltration surface is introduced.

---

## 20. Decisions

**Resolved:**
- Score engine: **hybrid** (deterministic anchor + pinned LLM weighting + narration).
- Research: **external frontier + own corpus**, as Phase 0, producing a versioned rubric.
- Outcome signal: **composite of computed signals**, anchored by hard markers where present.
- Scope: **Full DOSSIER** (all 7 dimensions + heatmap + score).
- Embeddings: **on-device** (fastembed + sqlite-vec).
- Build trigger: **on-demand + cached** (no scheduler in v1).
- Isolation: **separate `dossier` branch in a worktree; reads existing data, writes only `dossier_*` tables.**

**Remaining (resolve during implementation):**
- Exact metric families + weights — **output of Phase 0 research.**
- Profile output schema final shape (how structured vs narrative per dimension).
- Whether archetype classification is embedding-clustered or signal-heuristic (decide in Phase 4
  against real data).
- Milestone number for DOSSIER.
