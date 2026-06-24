//! RADAR context size + composition (pure).
//!
//! "Size" is the live context-window *occupancy* — the tokens currently resident
//! in the model's window, taken from the last assistant turn's usage. It is NOT
//! lifetime tokens burned: it deflates when occupancy drops (e.g. a compaction).
//!
//! * **Claude** occupancy = `input + cache_creation + cache_read` (the cached and
//!   freshly-written prompt tokens that make up the resident prompt). `output` is
//!   the model's reply for that turn, not resident context, so it is excluded from
//!   the size but reported in the exact composition.
//! * **Codex** occupancy = `last_token_usage.input_tokens` (already the resident
//!   prompt size); the max window comes from `task_started.model_context_window`.
//!
//! Exact composition is the API-anchored split of the resident context:
//! `cache_read` (cache-stable) vs `fresh` (`input + cache_creation`, freshly
//! written this turn) vs `output`.

use crate::ir::Event;

/// Live context occupancy: resident tokens, the model's max window, and the
/// clamped fill ratio.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextSize {
    pub context_tokens: u64,
    pub max_tokens: u64,
    pub fill_pct: f64,
}

/// API-anchored split of the resident context (exact, from the transcript usage).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExactComposition {
    pub cache_read: u64,
    pub fresh: u64,
    pub output: u64,
}

/// Clamp `tokens / max` to `[0, 1]`; `0.0` when `max == 0` (unknown window).
fn fill_pct(tokens: u64, max: u64) -> f64 {
    if max == 0 {
        return 0.0;
    }
    (tokens as f64 / max as f64).clamp(0.0, 1.0)
}

/// Claude context size from the last assistant turn's `TokenUsage`.
/// `context_tokens = input + cache_creation + cache_read`; the max window is
/// resolved from the model id. A non-`TokenUsage` event yields a zeroed size.
pub fn claude_context_size(last_usage: &Event, model: &str) -> ContextSize {
    let context_tokens = match last_usage {
        Event::TokenUsage {
            input,
            cache_creation,
            cache_read,
            ..
        } => *input as u64 + *cache_creation as u64 + *cache_read as u64,
        _ => 0,
    };
    let max_tokens = max_window_for_model(model);
    ContextSize {
        context_tokens,
        max_tokens,
        fill_pct: fill_pct(context_tokens, max_tokens),
    }
}

/// Codex context size: `input_tokens` is already the resident prompt size, and the
/// max window is `task_started.model_context_window` (0 ⇒ unknown ⇒ fill 0).
pub fn codex_context_size(input_tokens: u64, model_context_window: u64) -> ContextSize {
    ContextSize {
        context_tokens: input_tokens,
        max_tokens: model_context_window,
        fill_pct: fill_pct(input_tokens, model_context_window),
    }
}

/// Exact composition from a `TokenUsage` event: `cache_read` (cache-stable),
/// `fresh = input + cache_creation` (written this turn), and `output`. A
/// non-`TokenUsage` event yields all zeros.
pub fn exact_composition(last_usage: &Event) -> ExactComposition {
    match last_usage {
        Event::TokenUsage {
            input,
            output,
            cache_creation,
            cache_read,
            ..
        } => ExactComposition {
            cache_read: *cache_read as u64,
            fresh: *input as u64 + *cache_creation as u64,
            output: *output as u64,
        },
        _ => ExactComposition {
            cache_read: 0,
            fresh: 0,
            output: 0,
        },
    }
}

/// Estimated (semantic) composition: a local-tokenizer breakdown of the resident
/// context into honest, labeled buckets. NOT API-anchored at the bucket level —
/// the UI must label these "est." (see the design spec §4.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstComposition {
    pub preamble: u64,
    pub conversation: u64,
    pub tool_output: u64,
    pub thinking: u64,
}

/// Estimate the semantic composition, calibrated so the four labeled buckets sum
/// to EXACTLY `exact_total` (the current API-reported occupancy). This keeps the
/// estimate honest: ratios come from local tokenization, but the total matches the
/// ground truth rather than drifting.
///
/// * `preamble = turn1_total − first_user_tokens` (system + tools + memory, one
///   block ≈ the first assistant turn's input minus the first user message). It is
///   the firmest estimate — the turn-1 baseline — so it is held FIXED;
/// * the remaining `exact_total − preamble` is distributed across
///   `[conversation, tool_output, thinking]` in proportion to their raw token
///   counts, with the integer rounding remainder folded into `conversation` (the
///   largest of the three) so the total is exact.
///
/// Degenerate inputs are handled: when there is no turn-1 baseline AND no other
/// signal (raw sum 0), or `exact_total == 0`, all buckets are zero. When the fixed
/// preamble alone meets/exceeds `exact_total`, the other three are zero and
/// preamble is clamped to the anchor (the resident context is essentially all
/// preamble).
pub fn estimate_composition(
    turn1_total: u64,
    first_user_tokens: u64,
    conversation_tokens: u64,
    tool_output_tokens: u64,
    thinking_tokens: u64,
    exact_total: u64,
) -> EstComposition {
    let preamble = turn1_total.saturating_sub(first_user_tokens);
    let rest_raw = [conversation_tokens, tool_output_tokens, thinking_tokens];
    let rest_raw_sum: u64 = rest_raw.iter().sum();

    if exact_total == 0 || (preamble == 0 && rest_raw_sum == 0) {
        return EstComposition {
            preamble: 0,
            conversation: 0,
            tool_output: 0,
            thinking: 0,
        };
    }

    // Preamble is held fixed but never exceeds the exact anchor (the resident
    // context cannot be more preamble than it holds in total).
    let preamble = preamble.min(exact_total);
    let remaining = exact_total - preamble;

    if remaining == 0 || rest_raw_sum == 0 {
        // All anchor consumed by preamble (or no other signal) → rest is zero.
        return EstComposition {
            preamble: exact_total, // fill the anchor exactly
            conversation: 0,
            tool_output: 0,
            thinking: 0,
        };
    }

    // Distribute `remaining` across the three buckets in proportion to raw tokens.
    let scale = remaining as f64 / rest_raw_sum as f64;
    let mut rest: [u64; 3] = [0; 3];
    for (i, &r) in rest_raw.iter().enumerate() {
        rest[i] = (r as f64 * scale).round() as u64;
    }
    // Fold the rounding remainder into conversation so the three sum to `remaining`.
    let rest_sum: u64 = rest.iter().sum();
    if rest_sum < remaining {
        rest[0] += remaining - rest_sum;
    } else if rest_sum > remaining {
        rest[0] = rest[0].saturating_sub(rest_sum - remaining);
    }

    EstComposition {
        preamble,
        conversation: rest[0],
        tool_output: rest[1],
        thinking: rest[2],
    }
}

/// Process-wide cache of the `o200k_base` BPE encoder.
///
/// Building this encoder reconstructs the full ~50k-pair merge table from scratch
/// (~73% of a radar recompute's CPU when done per call), so it MUST be built once
/// and reused. `CoreBPE` is `Send + Sync` (plain hash maps + thread-local regex,
/// see `tiktoken_rs`'s own `o200k_base_singleton`), so a `&'static` is shared
/// across the recompute worker and any other caller. `None` caches a vocabulary
/// load failure so it is not retried on every call (the fallback heuristic is used).
static O200K_ENCODER: std::sync::OnceLock<Option<tiktoken_rs::CoreBPE>> =
    std::sync::OnceLock::new();
/// How many times the encoder has actually been constructed — must stay 1 for the
/// life of the process. Incremented exactly once, inside the `OnceLock` initializer.
static O200K_ENCODER_INITS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// Per-thread count of `tokenize_len` invocations. THREAD-LOCAL (not a global
/// atomic) so the radar incremental-cache tests can measure tokenization caused by
/// their own synchronous `assemble` call without pollution from other test threads
/// running in parallel in the same process. `assemble` tokenizes on the calling
/// thread, so the thread-local delta is exactly that assemble's tokenizations.
#[cfg(test)]
thread_local! {
    static TOKENIZE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Test probe: `tokenize_len` invocations on THIS thread so far (compare deltas).
#[cfg(test)]
pub(crate) fn tokenize_call_count() -> usize {
    TOKENIZE_CALLS.with(|c| c.get())
}

/// The cached `o200k_base` encoder (`None` if the vocabulary failed to load once).
/// Builds it on first call and reuses the same instance thereafter.
fn encoder() -> Option<&'static tiktoken_rs::CoreBPE> {
    O200K_ENCODER
        .get_or_init(|| {
            // Count the single real construction so a test can assert "built once".
            O200K_ENCODER_INITS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            tiktoken_rs::o200k_base().ok()
        })
        .as_ref()
}

/// Test/diagnostic probe: how many times the cached encoder has been constructed.
/// Stays 1 once warmed — proves the per-call reconstruction is gone.
#[cfg(test)]
fn encoder_init_count() -> usize {
    O200K_ENCODER_INITS.load(std::sync::atomic::Ordering::SeqCst)
}

/// Approximate token count of `text` via a local BPE tokenizer (`o200k_base`, the
/// GPT-4o/o-series vocabulary — the closest offline approximation to modern model
/// tokenization). Used to derive the turn-1 preamble baseline and to size large
/// `tool_result` blocks for the estimated composition; calibrated against the
/// exact API totals upstream so tokenizer drift is bounded.
///
/// Uses the process-wide cached [`encoder`] (built once) with the SAME
/// `encode_with_special_tokens` method as before — token counts are byte-identical;
/// only the redundant per-call encoder reconstruction is removed.
pub fn tokenize_len(text: &str) -> u64 {
    #[cfg(test)]
    TOKENIZE_CALLS.with(|c| c.set(c.get() + 1));
    match encoder() {
        Some(bpe) => bpe.encode_with_special_tokens(text).len() as u64,
        // Fallback to a coarse chars/4 heuristic if the vocabulary fails to load.
        None => (text.chars().count() as u64).div_ceil(4),
    }
}

/// Max context window for a model id, by substring match. Returns `0` for an
/// unknown model so `fill_pct` degrades to `0.0` (honest: no fabricated window).
///
/// Table (per the M3 design spec §4.5 and the model lookup anchor):
/// * Claude `opus`/`sonnet`/`haiku` → 200_000 (the 1M-context Sonnet variant is
///   still reported at the default 200k unless explicitly a `-1m` id);
/// * Codex / GPT-5-class → 258_400.
pub fn max_window_for_model(model: &str) -> u64 {
    let m = model.to_ascii_lowercase();
    // Explicit 1M-context Sonnet variant, when the id advertises it. Match only the
    // two real forms (`-1m` suffix, `[1m]` beta tag) — a bare `1m` substring is too
    // broad (it would catch e.g. a date fragment like `21mar`).
    if m.contains("sonnet") && (m.contains("-1m") || m.contains("[1m]")) {
        return 1_000_000;
    }
    if m.contains("opus") || m.contains("sonnet") || m.contains("haiku") || m.contains("claude") {
        return 200_000;
    }
    // `openai` is the Codex Desktop *provider* id (`session_meta.model_provider`),
    // which `build_agent` passes here for Codex sessions — map it to the Codex/GPT-5
    // class window so the live Codex globe gets a real context bar instead of 0.
    if m.contains("codex")
        || m.contains("gpt-5")
        || m.contains("gpt5")
        || m.contains("o200k")
        || m.contains("openai")
    {
        return 258_400;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, cache_creation: u32, cache_read: u32, output: u32, model: &str) -> Event {
        Event::TokenUsage {
            input,
            output,
            cache_creation,
            cache_read,
            model: model.to_string(),
            orchestration: None,
        }
    }

    /// Claude size = input+cache_creation+cache_read (NOT output); fill clamps to 1
    /// when occupancy exceeds the window.
    #[test]
    fn claude_context_size_sums_resident_and_clamps_fill() {
        let u = usage(2, 13761, 331244, 2620, "claude-opus-4-8");
        let size = claude_context_size(&u, "claude-opus-4-8");
        assert_eq!(size.context_tokens, 345_007, "2 + 13761 + 331244");
        assert_eq!(size.max_tokens, 200_000);
        assert!(
            (size.fill_pct - 1.0).abs() < 1e-9,
            "345007/200000 clamps to 1.0, got {}",
            size.fill_pct
        );
    }

    /// Exact composition splits the resident context: cache_read, fresh
    /// (=input+cache_creation), output.
    #[test]
    fn exact_composition_splits_cache_fresh_output() {
        let u = usage(2, 13761, 331244, 2620, "claude-opus-4-8");
        let c = exact_composition(&u);
        assert_eq!(c.cache_read, 331_244);
        assert_eq!(c.fresh, 2 + 13_761);
        assert_eq!(c.output, 2_620);
    }

    /// Codex size uses input_tokens against the task_started window; ~0.57 fill.
    #[test]
    fn codex_context_size_computes_fill() {
        let size = codex_context_size(147_289, 258_400);
        assert_eq!(size.context_tokens, 147_289);
        assert_eq!(size.max_tokens, 258_400);
        assert!(
            (size.fill_pct - 0.57).abs() < 0.01,
            "147289/258400 ≈ 0.57, got {}",
            size.fill_pct
        );
    }

    /// Unknown model → window 0 → fill 0 (no fabricated window).
    #[test]
    fn unknown_model_yields_zero_window_and_zero_fill() {
        assert_eq!(max_window_for_model("mystery-model"), 0);
        let size = claude_context_size(&usage(10, 0, 0, 0, "mystery"), "mystery");
        assert_eq!(size.max_tokens, 0);
        assert_eq!(size.fill_pct, 0.0);
    }

    /// The model table covers the Claude family and the Codex/GPT-5 window.
    #[test]
    fn max_window_table_covers_known_models() {
        assert_eq!(max_window_for_model("claude-opus-4-8"), 200_000);
        assert_eq!(max_window_for_model("claude-3-5-haiku"), 200_000);
        assert_eq!(max_window_for_model("claude-sonnet-4-5"), 200_000);
        assert_eq!(max_window_for_model("gpt-5-codex"), 258_400);
    }

    /// The Codex provider id (`openai`, from `session_meta.model_provider`) maps to
    /// the Codex / GPT-5-class window. `build_agent` passes the *provider* id (not a
    /// per-turn model name) to `max_window_for_model` for Codex, so without an
    /// `openai` entry the live Codex globe reports a 0 window and shows no context
    /// bar. The on-disk `model_context_window` equals this value (258_400) today, so
    /// the table stays the single honest window source for every harness.
    #[test]
    fn max_window_codex_openai_provider_is_gpt5_class() {
        assert_eq!(max_window_for_model("openai"), 258_400);
        assert_eq!(max_window_for_model("OpenAI"), 258_400, "case-insensitive");
    }

    /// The 1M-context Sonnet window matches ONLY the two explicit advertised forms
    /// (`-1m` suffix and the `[1m]` beta tag); a bare `1m` substring that is not one
    /// of those forms must NOT widen the window to 1M (it stays the default 200k).
    #[test]
    fn max_window_1m_sonnet_matches_only_explicit_forms() {
        // Explicit forms → 1M.
        assert_eq!(max_window_for_model("claude-sonnet-4-5-1m"), 1_000_000);
        assert_eq!(max_window_for_model("claude-sonnet-4-5[1m]"), 1_000_000);
        // Bare "1m" substring that is not an explicit form → default 200k, not 1M.
        assert_eq!(
            max_window_for_model("claude-sonnet-4-5-21mar"),
            200_000,
            "a stray '1m' inside the id must not widen to 1M"
        );
    }

    /// Estimated composition: preamble = turn1_total − first_user; the four
    /// labeled buckets are scaled to sum to EXACTLY the exact anchor (12000).
    #[test]
    fn estimate_composition_calibrates_to_exact_total() {
        let est = estimate_composition(8000, 500, 3000, 1500, 200, 12000);
        assert_eq!(est.preamble, 7500, "preamble = turn1_total − first_user");
        let sum = est.preamble + est.conversation + est.tool_output + est.thinking;
        assert_eq!(sum, 12000, "buckets must sum to the exact anchor");
        // Ratios preserved (conversation > tool_output > thinking, all non-negative).
        assert!(est.conversation > est.tool_output);
        assert!(est.tool_output > est.thinking);
    }

    /// Calibration scales DOWN when the raw estimate exceeds the exact total, still
    /// summing to exactly the anchor.
    #[test]
    fn estimate_composition_scales_down_to_anchor() {
        // raw = 7500+3000+1500+200 = 12200 > exact 6000 → must scale down to 6000.
        let est = estimate_composition(8000, 500, 3000, 1500, 200, 6000);
        let sum = est.preamble + est.conversation + est.tool_output + est.thinking;
        assert_eq!(sum, 6000, "down-scaled buckets must still sum to the anchor");
        assert!(est.preamble <= 7500);
    }

    /// A zero raw sum (no turn-1 baseline) or zero anchor returns all zeros.
    #[test]
    fn estimate_composition_handles_degenerate_inputs() {
        assert_eq!(
            estimate_composition(0, 0, 0, 0, 0, 12000),
            EstComposition { preamble: 0, conversation: 0, tool_output: 0, thinking: 0 }
        );
        assert_eq!(
            estimate_composition(8000, 500, 3000, 1500, 200, 0),
            EstComposition { preamble: 0, conversation: 0, tool_output: 0, thinking: 0 }
        );
    }

    /// The local tokenizer returns a positive length for non-empty text and 0 for
    /// the empty string (sanity: wiring works, drift bounded by calibration).
    #[test]
    fn tokenize_len_is_positive_for_text() {
        assert!(tokenize_len("the quick brown fox jumps over the lazy dog") > 0);
        assert_eq!(tokenize_len(""), 0);
    }

    /// CORRECTNESS GUARD for the encoder-cache refactor: `tokenize_len` must remain
    /// byte-identical to a freshly-built `o200k_base()` encoder's
    /// `encode_with_special_tokens(text).len()` — the EXACT method the pre-cache code
    /// used. This includes text containing a special-token literal (`<|endoftext|>`),
    /// where `encode_with_special_tokens` (collapses it to ONE token) and
    /// `encode_ordinary` (splits it into several) DIVERGE — proving the cache keeps
    /// the same method and so the same counts.
    #[test]
    fn tokenize_len_matches_encode_with_special_tokens_exactly() {
        let bpe = tiktoken_rs::o200k_base().expect("o200k_base loads");
        for s in [
            "",
            "hello world",
            "the quick brown fox jumps over the lazy dog",
            "fn main() { println!(\"{}\", 42); }",
            "emoji 🦀 and unicode café — naïve",
            // Special-token literal: collapses to 1 token under special-token
            // encoding, but would be several tokens under encode_ordinary.
            "a prompt that ends with <|endoftext|> inside it",
        ] {
            let expected = bpe.encode_with_special_tokens(s).len() as u64;
            assert_eq!(
                tokenize_len(s),
                expected,
                "tokenize_len must equal encode_with_special_tokens len for {s:?}"
            );
        }
    }

    /// The expensive `o200k_base` encoder is constructed EXACTLY ONCE across many
    /// `tokenize_len` calls (this is the ~70% CPU win): the cache builds it on first
    /// use and reuses the same instance thereafter. Instrumented via the cache's
    /// `encoder_init_count()` probe.
    #[test]
    fn encoder_is_constructed_once_across_calls() {
        // Touch the encoder so it is initialized regardless of test ordering.
        let _ = tokenize_len("warm up the cache");
        let before = encoder_init_count();
        assert_eq!(before, 1, "encoder is built on first use");
        for _ in 0..500 {
            let _ = tokenize_len("the quick brown fox jumps over the lazy dog");
        }
        assert_eq!(
            encoder_init_count(),
            1,
            "the encoder must not be rebuilt on subsequent calls"
        );
    }

    /// A non-TokenUsage event yields a zeroed size and composition (defensive).
    #[test]
    fn non_token_usage_event_is_zeroed() {
        let e = Event::AssistantText {
            text: "hi".into(),
        };
        let size = claude_context_size(&e, "claude-opus-4-8");
        assert_eq!(size.context_tokens, 0);
        assert_eq!(exact_composition(&e), ExactComposition { cache_read: 0, fresh: 0, output: 0 });
    }
}
