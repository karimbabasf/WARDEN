# DOSSIER — Rubric Calibration (Phase 2a, empirical)

> Empirical calibration of `dossier-rubric-v1` against the **real** local corpus
> (`~/.warden/warden.db`, read-only). Answers the rubric's open questions with data.
> **Outcome: keep `RUBRIC_VERSION = dossier-rubric-v1` weights** (no re-weight — see Finding 1),
> fix the heatmap display scaling (in-scope), flag the featurizer subagent gap upstream.

## Corpus
- **2,152 sessions** (1,960 `claude_code`, 192 `codex`), **2,143 feature vectors**, **212,206 events**, **58 findings**.
- Window: **2026-06-05 → 2026-06-28** (~23 days).

## Signal distributions (percentiles over 2,143 FVs)
| signal | p10 | p25 | p50 | p75 | p90 | p95 | max |
|---|---|---|---|---|---|---|---|
| token_burn_total | 23k | 53k | 107k | 169k | 545k | 1.50M | 51.6M |
| context_saturation_peak | 0 | 0 | 0.001 | 0.189 | 0.408 | 0.762 | **256.6** |
| cache_read_ratio | 0.39 | 0.62 | 0.81 | 0.89 | 0.94 | 0.96 | 1.0 |
| search_in_main_context | 0 | 0 | 0 | 0 | 0 | 4 | 96 |
| **subagent_spawn_count** | 0 | 0 | 0 | 0 | 0 | **0** | **0** |
| **subagent_delegation_rate** | 0 | 0 | 0 | 0 | 0 | **0** | **0** |
| tool_error_rate | 0 | 0 | 0 | 0.09 | 0.17 | 0.25 | 1.0 |
| reprompt_count | 0 | 0 | 0 | 0 | 0 | 1 | 28 |
| prompt_specificity | 0.01 | 0.40 | 0.80 | 0.92 | 1.0 | 1.0 | 1.0 |
| file_churn | 0 | 0 | 0 | 0 | 0 | 3 | 271 |
| thrash_index | 0 | 0 | 0 | 0 | 1 | 3 | 16 |
| verification_present | — | — | — | — | — | **15% true** | — |

## Threshold trip-rates (rubric / live detectors, on real corpus)
| condition | trips |
|---|---|
| CONTEXT_BLOAT `search≥8` | 4% |
| `search≥4 & sat>0.35` | 4% |
| NO_DELEGATION `search≥3 & spawn==0` | 6% |
| WHACK_A_MOLE `thrash≥2 OR churn≥4` | 8% |
| CACHE_COLD `burn>20k & cache<0.08` | 1% |
| VAGUE `spec∈(0,0.28) & reprompt>0` | 4% |
| UNVERIFIED `tool_calls≥4 & !verify` | **66%** |
| `context_saturation_peak > 1.0` (overshoot) | 4% |

## Findings

### 1. CRITICAL — subagent signals are dead in the corpus (28% of score weight degenerate)
`subagent_spawn_count` and `subagent_delegation_rate` are **0 on every one of 2,143 sessions**.
The operator uses subagents heavily, so this is **not** real behavior — it is an **upstream
featurizer/adapter gap**: Task-tool subagent spawns are not being counted into the FeatureVector.
Consequences for the score:
- `right_sized_delegation` (weight 0.12) → its `spawn==0 ⇒ neutral 0.7` branch fires for *everyone*; the family is a **constant 0.7** and carries no signal.
- `delegation_hygiene` (weight 0.16) → its delegation-rate reward term is always 0; the family reduces to its `search_in_main_context` sub-term only.
**Decision: do NOT re-weight to compensate** (that would patch a symptom and hide the real bug, violating the WHACK_A_MOLE guardrail). Keep v1 weights — they become correct the moment the upstream signal is populated. Until then, **treat the two delegation families as low-confidence**; the live score is effectively carried by the other five families (verification, cache, context, rework, specificity — 0.62 of the weight).
**Remedy (out of DOSSIER's isolation scope):** fix `featurizer.rs` / the Claude adapter to record `SubagentSpawn` / `subagent_spawn_count`. Flagged as a separate task.

### 2. Verification is rare (15% present, 66% unverified-substantive) — threshold is correct, signal is real
Only 15% of sessions ran a test/build; 66% did ≥4 tool calls without verifying. `verification_discipline`
(0.16) therefore scores most sessions low — but this is a **true behavioral signal**, not a mis-set
threshold. Keep it. (It is arguably the single most honest efficiency signal in the corpus.)

### 3. context_saturation_peak is not a true occupancy fraction (confirmed)
p90 = 0.408 but **max = 256.6** and 4% exceed 1.0 — confirming the rubric caveat (it is
`cumulative_input / DEFAULT_WINDOW`). `context_discipline` floors the overshooters, so direction is
preserved, but the metric is noisy. **Future (upstream):** re-featurize from RADAR `fill_pct` (true
per-session window fraction). Kept as-is for v1.

### 4. Healthy, discriminating signals — keep absolute thresholds
- `cache_read_ratio` p50 = 0.81 (good hygiene); CACHE_COLD trips only 1%. `cache_efficiency` discriminates well.
- `prompt_specificity` p50 = 0.80, reprompts rare (p95 = 1). `prompt_specificity_yield` mostly high.
- Detector trip-rates (CONTEXT_BLOAT 4%, WHACK 8%, NO_DELEG 6%, VAGUE 4%, CACHE_COLD 1%) sit at sensible,
  non-degenerate frequencies. **Percentile-rank normalization is NOT needed yet** — the absolute thresholds
  cut at reasonable points on the real distribution. Revisit only if the corpus shifts.

### 5. Heatmap display — token outliers require robust scaling (in-scope fix, applied)
`token_burn_total` spans 23k (p10) → 51.6M (max); daily sums are even larger. Linear-to-max intensity
bucketing makes one heavy day wash every other day to the lowest level. **Fixed:** the heatmap intensity
now uses a **log scale** (robust to the long tail). This is the one rubric/UX change made in Phase 2a.

## Net decision
- **Rubric weights: unchanged (`dossier-rubric-v1`).** The one degeneracy (delegation) is an upstream
  data bug, not a weighting error; patching weights would hide it.
- **Heatmap scaling: log (applied this phase).**
- **Upstream follow-ups (outside DOSSIER):** (a) populate `subagent_spawn_count` in the featurizer/adapter;
  (b) re-featurize `context_saturation_peak` from RADAR `fill_pct`; (c) add commit / test-pass hard markers
  to strengthen the outcome signal. These are tracked separately.
