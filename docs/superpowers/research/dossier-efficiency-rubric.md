---
rubric_version: dossier-rubric-v1
generated_by: dossier-efficiency-research workflow (run wf_bac9be6c-6f2)
method: 8 dimension finders (web research) -> adversarial verification -> synthesis
---

# DOSSIER — Efficiency Rubric (v1)

> Source of truth for `rubric.rs` / `efficiency.rs`. `RUBRIC_VERSION = "dossier-rubric-v1"`.
> Weights below are the v1 anchor; the LLM may later *propose* revisions, pinned as v2 before use (spec §4).
> Every family maps to a real `FeatureVector` / detector signal and was adversarially verified.
> NOTE: the empirical corpus-mining pass (Phase 2) calibrates/validates these weights and the open questions below.

**Families:** 7 · **weight sum:** 1.000

| # | family | weight | dir | signals |
|---|---|---|---|---|
| 1 | `delegation_hygiene` | 0.16 | higher_better | search_in_main_context, subagent_delegation_rate, subagent_spawn_count, context_saturation_peak |
| 2 | `right_sized_delegation` | 0.12 | higher_better | subagent_spawn_count, token_burn_total, context_saturation_peak, file_churn, thrash_index, reprompt_count |
| 3 | `context_discipline` | 0.12 | lower_better | context_saturation_peak, token_burn_total |
| 4 | `cache_efficiency` | 0.15 | higher_better | cache_read_ratio, token_burn_total, reprompt_count |
| 5 | `verification_discipline` | 0.16 | higher_better | verification_present, tool_call_count, ignored_error_count, tool_error_rate, reprompt_count |
| 6 | `rework_and_thrash` | 0.11 | lower_better | thrash_index, file_churn, verification_present, ignored_error_count, tool_error_rate |
| 7 | `prompt_specificity_yield` | 0.18 | higher_better | prompt_specificity, reprompt_count, file_churn, thrash_index |

---

## 1. `delegation_hygiene`  — weight 0.16, higher_better

**Definition.** How well the operator keeps throwaway, read-heavy exploration (file reads, greps, log dumps) OUT of the main context by delegating it to subagents, so the main thread stays high-signal. Measures inline-vs-delegated exploration, not raw subagent volume. The canonical anti-pattern is heavy in-main searching with no delegation.

**Signals:** `search_in_main_context`, `subagent_delegation_rate`, `subagent_spawn_count`, `context_saturation_peak`

**Normalization.** Composite sub-score. (a) search_in_main_context: map to 0..1 via inverse absolute thresholds grounded in WARDEN's own live detectors (CONTEXT_BLOAT fires at >=8, and at >=4 with saturation>0.35): score = 1.0 at 0-2 searches, linearly decaying to 0.0 at 8 (clamp). (b) delegation_credit: when search_in_main_context>=3 AND subagent_spawn_count==0 (the NO_DELEGATION detector condition), cap this family at 0.4 regardless of other terms. (c) For sessions WITH delegation, reward subagent_delegation_rate in the healthy band 0.1-0.6 (rate above ~0.8 risks over-spawning, penalized by right_sized_delegation, so do not double-reward here). Final = 0.7*search_term + 0.3*delegation_term. Percentile-rank search_in_main_context across the operator's own sessions as a secondary calibration once a corpus exists, since absolute search counts scale with task size.

**Evidence basis.** Delegation & subagent usage P1 (corroborated, anchor); Context engineering P2; Orchestration P3; Token economics P4; Multi-agent interop P4. Grounded in Anthropic effective-context-engineering ('1,000-2,000 token distilled summary', 'essence of search is compression') and Cognition's longer-traces claim, all adversarially verified. WARDEN already operationalizes this via search_in_main_context (sidechain-aware, featurizer.rs) and the CONTEXT_BLOAT / NO_DELEGATION detectors.

**Weight rationale.** STRONGEST-evidenced dimension (triangulated across four independent sources) and the most directly, honestly computable signal in WARDEN (search_in_main_context already excludes sidechain/delegated work). High weight justified, but not dominant because it must be balanced against right-sizing so it cannot be gamed by 'always delegate'.

---

## 2. `right_sized_delegation`  — weight 0.12, higher_better

**Definition.** Whether delegation/fan-out is matched to workload size and independence, rather than over-spawning subagents for trivial work (paying coordination overhead with no benefit) or fanning out coupled work that then has to be reconciled. This family BOUNDS delegation_hygiene so 'delegate everything' cannot win.

**Signals:** `subagent_spawn_count`, `token_burn_total`, `context_saturation_peak`, `file_churn`, `thrash_index`, `reprompt_count`

**Normalization.** Two terms, multiplied. (a) over-spawn term: ratio r = subagent_spawn_count / (token_burn_total/10000). Score 1.0 when r is low (spawns scale with a genuinely large workload), decaying toward 0 when r is high (many spawns on a small/cheap workload). Concretely: r<=0.5 -> 1.0; r>=3 -> 0.2; linear between. Augment's 2-3-step crossover means the threshold is workload-dependent, so calibrate r empirically against the corpus rather than hard-coding. (b) reconciliation term: when subagent_spawn_count>0, penalize a POST-fan-out spike in file_churn+thrash_index+reprompt_count (coupled work reconciled): map (file_churn>=4 OR thrash_index>=2 OR reprompt_count>=3) co-occurring with spawns to a 0.4 multiplier; otherwise 1.0. Final = over_spawn_term * reconciliation_term. Sessions with subagent_spawn_count==0 are NEUTRAL (0.7) here, not penalized — a single-threaded small task is fine.

**Evidence basis.** Delegation & subagent usage P2 (coupled-work reconciliation, Cognition Flappy-Bird/Mario) and P3 (delegation floor / over-spawn, Augment worked token analysis, Claude Code docs). Orchestration P4 (fixed startup overhead, Owain Lewis 148k-token nested test). All corroborated; signals substituted to FeatureVector fields (RADAR concurrency/depth dropped as non-computable per-session).

**Weight rationale.** Essential guardrail on delegation_hygiene, but weighted lower because the over-spawn ratio is workload-dependent and noisy (Augment's crossover varies by task type), and per-subagent token attribution does not exist (token_burn_total is whole-session), so the signal is coarse.

---

## 3. `context_discipline`  — weight 0.12, lower_better

**Definition.** How well the operator keeps peak working-context small and high-signal rather than riding near the window limit, where model accuracy degrades ('context rot') and the whole resident prompt is re-billed each turn. Lower peak saturation = more time out of the degraded regime.

**Signals:** `context_saturation_peak`, `token_burn_total`

**Normalization.** Primary: context_saturation_peak, lower_better. MEASUREMENT CAVEAT — WARDEN computes it as cumulative_input/DEFAULT_WINDOW, which can exceed 1.0 on long sessions (it conflates total tokens processed with live-window fullness), so do NOT treat it as a literal occupancy fraction. Map: score 1.0 for peak<=0.4, linear decay to 0.2 at peak>=1.0, floor 0.1 above. The 0.8-0.9 'danger' band is an asserted practitioner heuristic (Augment >85% alert), not a measured cliff — use percentile-rank across the operator's own sessions as the PRIMARY normalization and the absolute curve only as a fallback for thin corpora. Where RADAR fill_pct (true per-session window fraction) can be joined, prefer it as the cleaner peak signal.

**Evidence basis.** Context engineering P1 (Chroma 18-model context-rot study, verified; Anthropic n^2 attention 'attention budget'; Manus degrade-beyond-a-length). Token economics P3. Note: Chroma stresses NON-UNIFORMITY, so the law is 'longer reliably tends to hurt, magnitude model/task-specific' — trim 'continuous'.

**Weight rationale.** Well-sourced with a rare controlled multi-model study behind it, and directly computable. Weighted moderately (not high) because WARDEN's saturation metric is an approximation (cumulative-over-window, can overshoot) and the threshold is heuristic, so it is a directional flag best paired with cache hygiene, not a precise standalone verdict.

---

## 4. `cache_efficiency`  — weight 0.15, higher_better

**Definition.** Fraction of re-sent resident-prompt input served from cache rather than re-billed as fresh tokens. Because the conversation is re-sent every turn (~100:1 input:output) and cached input is ~10x cheaper, a high cache-read ratio is the strongest single marker of a well-structured, prefix-stable agent run — CONDITIONAL on real work being done (a high ratio on an idle frozen-prompt session is not virtue).

**Signals:** `cache_read_ratio`, `token_burn_total`, `reprompt_count`

**Normalization.** Primary: cache_read_ratio directly as a 0..1 sub-score (it is already a fraction with the right denominator: cache_read/(input_total+cache_create+cache_read), output excluded). Apply WARDEN's live CACHE_COLD_RESTARTS basis: hard floor — if token_burn_total>20000 AND cache_read_ratio<0.08, force this family to <=0.15 (cache-busting / cold restarts). GATE against idle-session false positives: only count cache_read_ratio as virtue when the session shows real work (tool_call_count above a small floor or token_burn_total above a floor); an idle session re-reading a frozen prompt with high ratio is scored NEUTRAL, not high. Mid-session ModeChange (effort/model swap) events, where available, cap the score because each swap detonates the cached prefix.

**Evidence basis.** Context engineering P5 and Token economics P1+P2+P6 (all corroborated). Manus 'KV-cache hit rate is the single most important metric', verified $0.30 vs $3.00/MTok 10x, prefix-exact all-or-nothing invalidation (Anthropic docs, verified). WARDEN computes cache_read_ratio from real adapter fields and already ships CACHE_COLD_RESTARTS.

**Weight rationale.** Best-evidenced and most directly-computed efficiency signal in the whole set, with a live detector already. High weight. Slightly below delegation_hygiene because it is necessary-but-not-sufficient (can cache-hit a bloated prefix) and prices are Claude-specific, so it must be cross-checked against yield.

---

## 5. `verification_discipline`  — weight 0.16, higher_better

**Definition.** Whether the operator makes the agent validate work against external feedback (run tests/builds, read real output) before accepting it, and never trusts an unbacked 'done/fixed' claim. Captures both validation effort (share of trajectory spent verifying) and the unbacked-completion red flag.

**Signals:** `verification_present`, `tool_call_count`, `ignored_error_count`, `tool_error_rate`, `reprompt_count`

**Normalization.** Composite. (a) validation-present term: from WARDEN's UNVERIFIED_COMPLETION detector basis — if tool_call_count>=4 AND !verification_present, set term=0.1 (substantive session that never ran a test/build). If verification_present, term=1.0. Trivial sessions (tool_call_count<4) are NEUTRAL (0.7) — a one-line change needs no build. (b) unbacked-completion term: saw_done==true with verification_present==false is a red flag — multiply by 0.4. (c) ignored-error term: penalize ignored_error_count>0 and tool_error_rate>0.25 (the IGNORED_TOOL_ERROR detector), scaling 1.0->0.3. Final = validation_term * unbacked_term * error_term. Because verification_present is binary, prefer percentile-rank of a continuous validation-share proxy (verification-type bash calls / tool_call_count) once corpus-mined.

**Evidence basis.** Verification & validation P1 (arXiv:2604.02547 §4.2.2 validation effort rho=+0.50 p<0.001, verified; replicated arXiv:2511.00197), P2 (hallucinated completion — BSWEN cross-model 19 false positives, multiple arXiv corroborations), P3 (closed-loop autofix, Cognition/Devin + AgentForge). Rework P1 (external grader, Kamoi TACL 2024). Soften 'causal' to 'predictive' (observational-data confound flagged by the source).

**Weight rationale.** Among the most robustly multi-sourced dimensions, with the strongest replicated correlation in the corpus (read-before-edit rho=+0.68/-0.78) and three live WARDEN detectors. High weight. The 'done-without-verify' red flag is one of the few near-direct measures of hidden future rework.

---

## 6. `rework_and_thrash`  — weight 0.11, lower_better

**Definition.** Degree of unproductive churn: repeated edit/revert on the same file, repeated identical failing actions (doom-loops), and patching forward on a degraded state instead of resetting to a known-good root cause (WHACK_A_MOLE). Healthy sessions interleave each edit with a verify step so churn resolves; broken loops accumulate churn with no test run between attempts.

**Signals:** `thrash_index`, `file_churn`, `verification_present`, `ignored_error_count`, `tool_error_rate`

**Normalization.** Primary: thrash_index and file_churn, lower_better, anchored to WARDEN's live WHACK_A_MOLE detector (thrash_index>=2.0 OR file_churn>=4.0). Map: score 1.0 at thrash_index==0 AND file_churn<2; decay to 0.2 when thrash_index>=2 OR file_churn>=4. CONDITION on verification: high churn that is INTERLEAVED with verification_present (each edit followed by a run, churn resolving) is less penalized — multiply the penalty by 0.6 when verification_present is true, since execution-feedback-driven iteration is healthy. Unverified churn (high thrash + !verification_present) gets the full penalty. Percentile-rank across the operator's own sessions for calibration; absolute thresholds as cold-start fallback.

**Evidence basis.** Verification P5 (django-15863 28-syntax-errors-no-test-run exemplar). Rework P2 (doom-loops — OpenDev fingerprint/3-repetition detection, Self-Correction Bench 64.5% blind-spot), P3 (reset-to-root-cause / WHACK_A_MOLE — Anthropic git-revert-to-clean-state). All corroborated; WARDEN ships WHACK_A_MOLE on thrash_index/file_churn.

**Weight rationale.** Sound, computable, and backed by a live detector. Weighted moderately because the quantification is weaker (one qualitative exemplar plus mechanism; no measured effect size on a churn-without-verify ratio), and a clean 'revert-to-known-good vs ordinary churn' boundary needs commit/revert markers WARDEN does not yet detect.

---

## 7. `prompt_specificity_yield`  — weight 0.18, higher_better

**Definition.** Whether the operator front-loads specificity (bounded goal, constraints, concrete requirements, paths) so the first attempt lands, versus issuing fuzzy asks that trigger correction rounds. Captures the canonical 'low specificity + high re-prompt = thrash from a fuzzy ask' signature and the super-linear (triangular) re-billing cost of each correction round.

**Signals:** `prompt_specificity`, `reprompt_count`, `file_churn`, `thrash_index`

**Normalization.** Composite. (a) specificity term: prompt_specificity directly (0..1). Use WARDEN's VAGUE_PROMPT detector basis (specificity in (0,0.28) AND reprompt_count>0) as a hard low-score zone -> term<=0.3. (b) re-prompt coupling: penalize reprompt_count rising with downstream file_churn/thrash_index — score 1.0 at reprompt_count==0, decaying to 0.3 at reprompt_count>=4 with churn present. IMPORTANT honesty caveats baked in: prompt_specificity is a COARSE LEXICAL proxy (word count + path presence + acceptance keywords + backtick density) measured on USER prompts, not the delegation message, with a ~50x-wide odds-ratio CI in the source — so weight reprompt_count (the lagging rework confirmation) MORE heavily than prompt_specificity: Final = 0.35*specificity_term + 0.65*reprompt_term. Percentile-rank prompt_specificity across the operator's corpus rather than trusting its absolute scale.

**Evidence basis.** Prompt quality P1 (arXiv:2606.19644 Table 4 Specificity OR~66 for codegen at Gate 0, verified; scope to 'usable code produced', NOT merge), P3 (triangular re-billing cost, Augment, verified). Rework P6. Delegation P4 (instruction specificity gates handoff quality, Anthropic + MAST 41.8% specification-problems). NOTE: Prompt-quality P2 and P4, and Multi-agent-interop P6, were marked WEAK/keep-with-caveats and are NOT load-bearing here.

**Weight rationale.** Highest single weight because the lagging confirmation (reprompt_count) is a hard, reliable event count and the dimension is multiply-sourced (study + Anthropic + MAST). But the LEADING indicator (prompt_specificity) is the fuzziest field in WARDEN, so the weight is deliberately tilted toward reprompt_count inside the family rather than trusting the lexical specificity score.

---

## Outcome signal (the numerator — what efficiency is measured toward)

A composite, per-session "good session" label used to EMPIRICALLY validate (not define) the families against the operator's real corpus. A session scores as good when it shows yield-per-resource, NOT minimal resource. Constructed as a weighted combination of computable WARDEN signals: (1) PRODUCTIVE WORK PRESENT — verification_present==true (a test/build ran) AND saw_edit (files changed) AND saw_done (a completion declaration), with verification_present REQUIRED to gate against hallucinated completion; (2) LOW REWORK — thrash_index<2 and file_churn<4 (below the WHACK_A_MOLE detector thresholds), low reprompt_count, ignored_error_count==0 and tool_error_rate<0.25 (below IGNORED_TOOL_ERROR); (3) CACHE HYGIENE — cache_read_ratio above a corpus-relative median and NOT in the CACHE_COLD_RESTARTS zone (token_burn_total>20k with ratio<0.08); (4) RIGHT-SIZED DELEGATION & CONTEXT — search_in_main_context below the CONTEXT_BLOAT threshold, delegation present when tool_call_count is high (escapes NO_DELEGATION), and context_saturation_peak not riding extreme; (5) where transcript hard markers exist (commits, explicit test-pass output), treat them as a strong positive overlay. CRITICAL: token_burn_total is NEVER a standalone negative — high burn that co-occurs with completed verified work, edits that stick, and low rework is a GOOD session; high burn with thrash, ignored errors, re-prompts, and no verification is the bad one. The outcome label is intentionally independent-ish of the families (it leans on hard markers: verification_present, saw_done, commits, thrash/error counts) so that family weights can be regressed/calibrated against it without circularity, and any family that fails to predict it is down-weighted or dropped in the empirical pass.

## Open questions — for the Phase 2 empirical corpus-mining pass

- Per-subagent token attribution does not exist: token_burn_total is a whole-session sum and subagents are tracked only as a spawn COUNT. Can the ingest layer attribute tokens per sidechain/subagent so right_sized_delegation can measure tokens-per-subagent (Augment's crossover) instead of the coarse session-level ratio?
- RADAR concurrency and subagent hierarchy depth are LIVE-PRESENCE fields not joined to per-session FeatureVector scoring. Several read-vs-write fan-out and shallow-vs-deep-topology principles could only be approximated by churn proxies. Is it worth plumbing a per-session concurrency/depth feature from RADAR's persisted toolUseId linkage, and does it actually predict the outcome signal?
- context_saturation_peak is cumulative_input/DEFAULT_WINDOW and can exceed 1.0 — it is not true peak window occupancy. Should we re-featurize it from RADAR fill_pct (real per-session window fraction), and does the corrected metric change which sessions trip CONTEXT_BLOAT?
- prompt_specificity is a coarse lexical proxy measured on USER prompts, not the orchestrator's delegation message to subagents, with a ~50x-wide odds-ratio CI in the source. How reliably does it actually correlate with reprompt_count / file_churn on this operator's corpus, and should it be re-derived from delegation messages specifically?
- Hard markers are thin: WARDEN detects verification_present (test/build bash heuristic) and saw_done (text 'done/complete/fixed') but NOT commits or genuine test-PASS output. Can we add a git-commit and test-result-parse marker so the outcome signal is grounded in real success, not just a verbal claim?
- Absolute thresholds (CONTEXT_BLOAT>=8, WHACK_A_MOLE thrash>=2/churn>=4, CACHE_COLD_RESTARTS 20k/0.08) are practitioner heuristics. On the operator's real session distribution, do these cut at sensible points, or should each family switch to within-operator percentile-rank normalization?
- Cross-task-type noise: Augment's delegation crossover and validation share both vary by task type (codegen pushes thresholds later than file-reading). Does the corpus need per-archetype (e.g. greenfield vs debug vs refactor) calibration before family weights are trusted?
- The initial family weights (summing to ~1.0) are priors, not fitted. After labeling sessions with the outcome signal, which families actually carry predictive signal — and do any (e.g. right_sized_delegation given the coarse ratio) collapse and warrant down-weighting or merging?
- ModeChange (effort/model swap) events are computed as permission_friction-style counts but not yet used for cache-detonation detection. Are mid-session model/effort swaps frequent enough in this corpus to be worth a dedicated cache-invalidation signal under cache_efficiency?
- saw_done text-matching ('done/complete/fixed') is noisy and can fire on unrelated prose. How many false positives does it produce, and does it need tightening before it anchors the unbacked-completion red flag in verification_discipline?

## Evidence appendix — verified principles kept, per dimension

### Delegation & subagent usage  (5/5 kept)
- [corroborated] Delegate read-heavy, throwaway exploration (codebase/file search, log grepping, doc lookup) to subagents, and keep that work OUT of the main context. The win is compression and context-preservation: a subagent burns tens of thousands of tok…
- [corroborated] Delegation pays off when the subtasks are genuinely INDEPENDENT and breadth-first (parallel exploration / fan-out research / non-overlapping edits across unrelated files). When subtasks are coupled — they share state or one's output feeds a…
- [corroborated] There is a delegation floor: for short tasks (a quick fix, a focused question, ~2-3 steps), the fixed overhead of spawning — decomposition, routing, and synthesizing the subagent's result back — exceeds the context-isolation savings, so del…
- [corroborated] Delegation quality is gated by instruction specificity, not just the decision to delegate. The orchestrator must hand each subagent a clear objective, output format, tool/source guidance, and explicit task boundaries; vague one-liners ('res…
- [corroborated] Multi-agent delegation is a token-multiplier bet that only clears for high-value, parallelizable work; it is not free leverage. Agents already use ~4x the tokens of a chat, and multi-agent systems ~15x — and on browse/research evals token v…

### Context engineering & discipline  (6/6 kept)
- [corroborated] Context is a finite attention budget, not free space up to the window limit — model accuracy degrades as input tokens grow ('context rot'), so efficient operators keep the working context small and high-signal rather than filling it because…
- [corroborated] Delegate exploration/search to sub-agents so verbose throwaway tokens (file reads, logs, search dumps) stay in an isolated child context and only a small distilled summary (~1-2k tokens) returns to the main thread — running searches inline…
- [corroborated] Naive parallel fan-out of multiple coding sub-agents that don't share full context is fragile and a false economy: each sub-agent's actions encode implicit decisions, and conflicting decisions force merge/rework that exceeds the parallelism…
- [corroborated] On long-horizon tasks, proactively compact/reset context (summarize then reinitialize) and offload to external memory (files, notes, todo.md) instead of letting history accumulate unboundedly — reset between tasks rather than dragging a pol…
- [corroborated] Maximize cache reuse and stable context prefixes: in agentic loops cached input tokens are ~10x cheaper than uncached, and accumulating-context loops grow billed input tokens quadratically, so a high cache-read ratio is one of the strongest…
- [corroborated] Curate what enters context, but do NOT scrub failed actions and errors — keep error/observation evidence in context (so the model adapts and stops repeating the mistake) while pruning redundant/irrelevant tool output (distractors), because…

### Verification & validation loops  (5/5 kept)
- [corroborated] Operators who make the agent spend a meaningful share of its trajectory on validation (running tests/builds and reading the output) before accepting work resolve more tasks and waste less compute. Verification effort is a positive driver of…
- [corroborated] Agents reliably hallucinate completion — declaring 'all tests pass / fixed / verified' on code that still fails — so an efficient operator never trusts a verbal done-claim and instead requires evidence (the actual command + its output) or a…
- [corroborated] Closing the loop — feeding test/lint/CI/review failures back to the agent so it fixes its own output until clean — is worth spending extra tokens on. The verification loop trades a higher token bill for a large reduction in escaped defects…
- [corroborated] Verifying BEFORE editing — gathering context and reproducing the problem first instead of front-loading patches — is itself a verification discipline that prevents the most expensive failure: solving the wrong problem. Premature patching in…
- [corroborated] Repeated edit-and-revert churn on the same file WITHOUT verification runs between attempts is a thrash signature of a broken loop — the agent is guessing rather than using execution feedback to converge. Efficient sessions interleave each e…

### Orchestration topology & parallelism  (5/6 kept)
- [corroborated] Parallel fan-out helps READ-heavy work (broad search/retrieval over many independent directions) but hurts WRITE-heavy work (code edits), where independent subagents make conflicting implicit decisions that don't merge. Efficient operators…
- [corroborated] Token burn is simultaneously the strongest single predictor of task quality AND the primary cost of orchestration — so efficiency is tokens-spent-relative-to-task-value, not raw token count. Multi-agent topologies buy capacity at ~4x (singl…
- [corroborated] Delegating search/exploration to a subagent instead of running it in the main thread is a core efficiency move: the file reads, grep output, and dead-end exploration stay in the subagent's throwaway context and only a distilled summary retu…
- [corroborated] Spawning a subagent has fixed startup overhead (a fresh agent spends its first turns rebuilding local understanding), so parallelism only pays off above a duration/work threshold. Fanning out many tiny tasks is net-negative; fanning out a f…
- [corroborated] Flat/shallow orchestration topologies beat deep subagent nesting for most work: each level of nesting multiplies token cost and compounds with depth, and adds debugging opacity, while genuinely hierarchical tasks that need per-layer special…

### Token economics & cache efficiency  (6/6 kept)
- [corroborated] Cache-read ratio is the dominant lever on agent cost and latency: in an agent loop the resident prompt is re-sent every turn (Manus reports a ~100:1 input:output ratio), so the fraction of input served from cache — billed at ~0.1x of fresh…
- [corroborated] Cache invalidation is prefix-based and all-or-nothing: any change to a token before the cache breakpoint forces the model to reprocess the entire remainder at full price. The efficient operator keeps the prompt prefix STABLE and append-only…
- [corroborated] Context is a finite attention budget with diminishing returns ('context rot'), so packing the window is not free — it degrades the model. Efficient operators keep peak context saturation well below the limit, treating the window as a budget…
- [corroborated] Delegating exploration to sub-agents is a token-economy move, not just an organizational one: a sub-agent burns tens of thousands of tokens in its OWN isolated context and returns a 1–2k-token distilled summary, so the lead agent's resident…
- [corroborated] Useful-output-per-token beats raw token count as the efficiency measure — but only up to the point of waste. Token spend explains the bulk of performance variance (Anthropic measured ~80% of variance in browsing evals from token usage), yet…
- [corroborated] Cache hygiene in practice = front-load the stable, append the volatile, and reset at task boundaries. Concretely: lock model/effort and read slow-changing large files EARLY (they become the warm prefix); touch edit-heavy files and inject pe…

### Prompt quality & specificity  (5/5 kept)
- [corroborated] Specificity is the single strongest determinant of whether a prompt yields actionable, usable code on the first pass rather than explanatory back-and-forth. Implementation-oriented prompts that state a bounded goal, explicit constraints, an…
- [weak] Underspecified prompts are not just slower, they are unstable: when a model must guess the missing requirement it only infers it correctly a minority of the time, and the resulting solution is roughly twice as likely to silently regress. Fo…
- [corroborated] Every re-prompt is disproportionately expensive because agent loops re-bill the entire accumulated conversation on each turn, so token cost grows like a triangular series, not linearly. A fuzzy opening prompt that triggers N correction roun…
- [weak] When a specified outcome is genuinely uncertain, a small number of up-front clarification turns is cheaper than letting the agent assume and rework. The efficient operator front-loads a bounded burst of clarification (or the agent asks) and…
- [corroborated] Specificity carried by reusable context (CLAUDE.md / AGENTS.md, slash commands, plan docs) is more durable than specificity re-typed per prompt: persistent project-level specification raises the first-attempt success rate and shrinks per-ta…

### Multi-agent interoperability  (6/6 kept)
- [corroborated] Spawn parallel agents only for genuinely independent, breadth-first work; for tasks with shared context or tight inter-step dependencies (most coding), a single agent thread is more efficient. Multi-agent burns ~15x the tokens of a chat (~4…
- [corroborated] Every delegated handoff must carry an explicit task contract — objective, output format, allowed tools/sources, and clear boundaries. Vague handoffs (passing only a short task string) cause subagents to duplicate each other's work, leave ga…
- [corroborated] Information must be compressed on the way UP the hierarchy: a subagent should explore in its own context (tens of thousands of tokens) but return only a distilled summary (~1,000-2,000 tokens), keeping raw tool output out of the lead agent'…
- [corroborated] Keep heavy exploration (search, multi-file reads) OUT of the main/orchestrator context — delegate it so the lead thread stays lean. Running searches inline in the main context pollutes the orchestrator's window with raw bytes, raising satur…
- [corroborated] Handoffs are lossy by default, and what gets dropped (constraints, rejected approaches, implicit decisions, API/interface requirements) resurfaces downstream as re-instruction and rework. The dominant multi-agent failure class is inter-agen…
- [weak] Cache the stable shared-context surface that every agent re-reads (system prompt, shared contract/AGENTS.md/CLAUDE.md, repo map). Because the same shared context is re-sent on every agent turn, an efficient multi-agent setup reuses it from…

### Rework, thrash & error recovery  (6/6 kept)
- [corroborated] Efficient operators route error correction through EXTERNAL feedback signals (run the test/build/LSP, read the real failure) rather than letting the agent 'self-correct' from intuition. Models reliably fix errors only when given a reliable…
- [corroborated] Doom-loops are the signature failure of inefficient operators: re-issuing a near-identical action after the same error (same edit, same command, same failing args) instead of changing strategy. Efficient operators/harnesses cap retries on a…
- [corroborated] Efficient operators recover by resetting to a known-good root cause (revert to last good commit / restart from a clean context) instead of stacking more patches on a broken state — the WARDEN WHACK_A_MOLE anti-pattern. Continuing to edit a…
- [corroborated] Running multiple WRITE agents in parallel manufactures rework: parallel agents make conflicting implicit decisions (style, edge-case handling, code patterns) that must later be reconciled or torn out. Efficient operators keep concurrent sub…
- [corroborated] Context rot drives re-explanation and repeated work: as a session's context grows large/noisy, the agent loses track of what it already tried, forcing the operator to re-instruct (reprompt) and the agent to redo work. Efficient operators br…
- [corroborated] Under-specified prompts are a leading cause of reprompt thrash: when the operator doesn't prescribe key decisions upfront, the agent guesses, the operator corrects, and the agent reworks — a cheap-to-avoid loop. Efficient operators front-lo…

