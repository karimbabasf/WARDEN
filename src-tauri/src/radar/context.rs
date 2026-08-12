//! Context-window breakdown + cost estimation (pure, calibrated to the live exact
//! total). Tokenizes a session's transcript into a semantic composition (cached by
//! content hash), splits it into renderable per-category rows, and prices the turn.

use super::composition::{estimate_composition, tokenize_len, ContextSize, ExactComposition};
use super::model::{RadarContextBreakdown, RadarContextRow, RadarEstimated};
use super::pricing;
use crate::ir::{Event, Harness, Session, ToolKind};
use crate::store::Store;
use std::collections::HashMap;

/// Compute the raw, pre-calibration token sums for a session's estimated
/// composition (the EXPENSIVE part — it tokenizes the transcript). `None` when there
/// is no turn-1 `TokenUsage` baseline (checked FIRST, so a baseline-less session does
/// zero tokenization). These sums depend only on transcript content, so they are
/// safely cacheable by content hash (Fix #3).
fn compute_token_counts(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
) -> Option<crate::store::RadarTokenCounts> {
    // turn-1 total = first TokenUsage's resident size (input+cache_read+cache_creation).
    let turn1_total = events.iter().find_map(|(_, e)| match &e.event {
        Event::TokenUsage {
            input,
            cache_creation,
            cache_read,
            ..
        } => Some(*input as u64 + *cache_creation as u64 + *cache_read as u64),
        _ => None,
    })?;

    let first_user_tokens = events
        .iter()
        .find_map(|(_, e)| match &e.event {
            Event::UserPrompt { text, .. } => Some(tokenize_len(text)),
            _ => None,
        })
        .unwrap_or(0);

    let mut conversation = 0u64;
    let mut tool_output = 0u64;
    let mut thinking = 0u64;
    for (_, e) in events {
        match &e.event {
            Event::AssistantText { text, .. } => conversation += tokenize_len(text),
            Event::UserPrompt { text, .. } => conversation += tokenize_len(text),
            // ToolResult byte size as a coarse token proxy (≈ bytes/4); the
            // calibration step rescales it to the exact anchor anyway.
            Event::ToolResult { bytes, .. } => tool_output += bytes / 4,
            Event::Thinking { tokens } => thinking += *tokens as u64,
            _ => {}
        }
    }

    Some(crate::store::RadarTokenCounts {
        turn1_total,
        first_user_tokens,
        conversation,
        tool_output,
        thinking,
    })
}

/// Derive the estimated (semantic) composition for a session, calibrated to its
/// current exact total. `None` when there is no turn-1 `TokenUsage` baseline.
///
/// Fix #3 (incremental): the expensive raw token sums are cached by
/// `(session id, content hash)`. A cache HIT skips re-tokenizing entirely; a MISS
/// (new or changed transcript) tokenizes once and upserts. The cheap, deterministic
/// `estimate_composition` calibration to the LIVE `exact_total` is always applied
/// fresh — so the output is byte-identical to tokenizing every time, only the
/// redundant tokenization is removed. Best-effort: a cache read/write failure simply
/// falls back to tokenizing (the value is never wrong, only recomputed).
pub(crate) fn estimate_for_session(
    store: &Store,
    session: &Session,
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    exact_total: u64,
) -> Option<RadarEstimated> {
    // The content hash is the change-key: it advances whenever the transcript is
    // re-ingested with new bytes, invalidating the cache for exactly that session.
    let change_key = session.raw_hash;

    let counts = match store.radar_token_cache_get(&session.id, change_key) {
        Ok(Some(hit)) => hit, // unchanged transcript → reuse, no tokenization
        _ => {
            // Miss (or read error): tokenize once, then persist under the change-key.
            let fresh = compute_token_counts(events)?;
            let _ = store.radar_token_cache_put(&session.id, change_key, &fresh);
            fresh
        }
    };

    let est = estimate_composition(
        counts.turn1_total,
        counts.first_user_tokens,
        counts.conversation,
        counts.tool_output,
        counts.thinking,
        exact_total,
    );
    Some(RadarEstimated {
        preamble: est.preamble,
        conversation: est.conversation,
        tool_output: est.tool_output,
        thinking: est.thinking,
    })
}

#[derive(Default)]
struct ToolContextStats {
    mcp_count: u32,
    system_count: u32,
    custom_count: u32,
    mcp_raw: u64,
    system_raw: u64,
    custom_raw: u64,
}

pub(crate) fn context_breakdown(
    harness: Harness,
    size: ContextSize,
    estimated: Option<RadarEstimated>,
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    pending_tail_tokens: u64,
) -> RadarContextBreakdown {
    let used = size.context_tokens;
    let max = size.max_tokens;
    let rows = match estimated {
        Some(est) => match harness {
            Harness::Codex => codex_context_rows(max, used, &est, events, pending_tail_tokens),
            _ => claude_context_rows(max, used, &est, events, pending_tail_tokens),
        },
        None => fallback_context_rows(max, used, pending_tail_tokens),
    };

    RadarContextBreakdown {
        used_tokens: used,
        max_tokens: max,
        fill_pct: size.fill_pct,
        rows,
    }
}

fn fallback_context_rows(max: u64, used: u64, pending_tail_tokens: u64) -> Vec<RadarContextRow> {
    let base = used.saturating_sub(pending_tail_tokens);
    let mut rows = vec![context_row("context", "Context", base, max, None, false)];
    append_pending_and_free_rows(&mut rows, max, used, pending_tail_tokens);
    rows
}

fn claude_context_rows(
    max: u64,
    used: u64,
    est: &RadarEstimated,
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    pending_tail_tokens: u64,
) -> Vec<RadarContextRow> {
    let tools = tool_context_stats(events, est.tool_output);
    let memory_count = memory_file_count(events);
    let (skills, memory, system_prompt, deferred_mcp, deferred_system) = split_preamble_for_claude(
        est.preamble,
        memory_count,
        tools.mcp_count,
        tools.system_count,
    );

    let mut rows = vec![
        context_row("messages", "Messages", est.conversation, max, None, false),
        context_row("skills", "Skills", skills, max, None, false),
        context_row(
            "mcp_tools",
            "MCP tools",
            tools.mcp_raw,
            max,
            count_if_nonzero(tools.mcp_count),
            false,
        ),
        context_row(
            "memory_files",
            "Memory files",
            memory,
            max,
            count_if_nonzero(memory_count),
            false,
        ),
        context_row(
            "system_prompt",
            "System prompt",
            system_prompt,
            max,
            None,
            false,
        ),
        context_row(
            "system_tools",
            "System tools",
            tools.system_raw,
            max,
            count_if_nonzero(tools.system_count),
            false,
        ),
        context_row(
            "custom_agents",
            "Custom agents",
            tools.custom_raw,
            max,
            count_if_nonzero(tools.custom_count),
            false,
        ),
        context_row(
            "mcp_tools_deferred",
            "MCP tools (deferred)",
            deferred_mcp,
            max,
            None,
            true,
        ),
        context_row(
            "system_tools_deferred",
            "System tools (deferred)",
            deferred_system,
            max,
            None,
            true,
        ),
    ];
    append_pending_and_free_rows(&mut rows, max, used, pending_tail_tokens);
    rows
}

fn codex_context_rows(
    max: u64,
    used: u64,
    est: &RadarEstimated,
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    pending_tail_tokens: u64,
) -> Vec<RadarContextRow> {
    let tools = tool_context_stats(events, est.tool_output);
    let mut rows = vec![
        context_row("messages", "Messages", est.conversation, max, None, false),
        context_row("reasoning", "Reasoning", est.thinking, max, None, false),
        context_row(
            "function_tools",
            "Function tools",
            tools.system_raw,
            max,
            count_if_nonzero(tools.system_count),
            false,
        ),
        context_row(
            "mcp_tools",
            "MCP tools",
            tools.mcp_raw,
            max,
            count_if_nonzero(tools.mcp_count),
            false,
        ),
        context_row(
            "custom_tools",
            "Custom tools",
            tools.custom_raw,
            max,
            count_if_nonzero(tools.custom_count),
            false,
        ),
        context_row(
            "base_instructions",
            "Base instructions",
            est.preamble,
            max,
            None,
            false,
        ),
    ];
    append_pending_and_free_rows(&mut rows, max, used, pending_tail_tokens);
    rows
}

fn append_pending_and_free_rows(
    rows: &mut Vec<RadarContextRow>,
    max: u64,
    used: u64,
    pending_tail_tokens: u64,
) {
    if pending_tail_tokens > 0 {
        rows.push(context_row(
            "pending_tail",
            "Pending tail (est.)",
            pending_tail_tokens,
            max,
            None,
            false,
        ));
    }
    if max > 0 {
        rows.push(context_row(
            "free_space",
            "Free space",
            max.saturating_sub(used),
            max,
            None,
            true,
        ));
    }
}

fn context_row(
    key: &str,
    label: &str,
    tokens: u64,
    max: u64,
    count: Option<u32>,
    muted: bool,
) -> RadarContextRow {
    RadarContextRow {
        key: key.to_string(),
        label: label.to_string(),
        tokens,
        percent: if max == 0 {
            0.0
        } else {
            (tokens as f64 / max as f64).clamp(0.0, 1.0)
        },
        count,
        muted,
    }
}

fn count_if_nonzero(n: u32) -> Option<u32> {
    (n > 0).then_some(n)
}

fn split_preamble_for_claude(
    preamble: u64,
    memory_count: u32,
    mcp_count: u32,
    system_count: u32,
) -> (u64, u64, u64, u64, u64) {
    if preamble == 0 {
        return (0, 0, 0, 0, 0);
    }
    let weights = [
        4 + u64::from(mcp_count > 0),
        1 + memory_count as u64,
        5,
        u64::from(mcp_count),
        u64::from(system_count),
    ];
    let split = allocate_by_weights(preamble, weights);
    (split[0], split[1], split[2], split[3], split[4])
}

fn allocate_by_weights<const N: usize>(total: u64, weights: [u64; N]) -> [u64; N] {
    let sum: u64 = weights.iter().sum();
    if total == 0 || sum == 0 {
        return [0; N];
    }
    let mut out = [0; N];
    let mut assigned = 0u64;
    for (i, weight) in weights.iter().enumerate() {
        out[i] = total.saturating_mul(*weight) / sum;
        assigned = assigned.saturating_add(out[i]);
    }
    if assigned < total {
        out[0] = out[0].saturating_add(total - assigned);
    }
    out
}

fn tool_context_stats(
    events: &[(crate::ir::Turn, crate::ir::EventRecord)],
    tool_output_tokens: u64,
) -> ToolContextStats {
    let mut call_kind: HashMap<String, ToolKind> = HashMap::new();
    let mut stats = ToolContextStats::default();
    let mut raw = [0u64; 3];

    for (_, e) in events {
        match &e.event {
            Event::ToolCall { call_id, kind, .. } => {
                call_kind.insert(call_id.clone(), kind.clone());
                match kind {
                    ToolKind::Mcp => stats.mcp_count += 1,
                    ToolKind::SubagentTask => stats.custom_count += 1,
                    _ => stats.system_count += 1,
                }
            }
            Event::ToolResult { call_id, bytes, .. } => {
                match call_kind.get(call_id).unwrap_or(&ToolKind::Unknown) {
                    ToolKind::Mcp => raw[0] += *bytes,
                    ToolKind::SubagentTask => raw[2] += *bytes,
                    _ => raw[1] += *bytes,
                }
            }
            _ => {}
        }
    }

    if raw.iter().all(|n| *n == 0) {
        raw = [
            u64::from(stats.mcp_count),
            u64::from(stats.system_count),
            u64::from(stats.custom_count),
        ];
    }
    let split = allocate_by_weights(tool_output_tokens, raw);
    stats.mcp_raw = split[0];
    stats.system_raw = split[1];
    stats.custom_raw = split[2];
    stats
}

fn memory_file_count(events: &[(crate::ir::Turn, crate::ir::EventRecord)]) -> u32 {
    let count = events
        .iter()
        .map(|(_, e)| match &e.event {
            Event::FileSnapshot { files } => files.len(),
            Event::UserPrompt { attachments, .. } => attachments.len(),
            _ => 0,
        })
        .sum::<usize>();
    count.min(u32::MAX as usize) as u32
}

/// Rough USD cost for the turn's tokens, from the model pricing table in
/// `pricing.rs`. `None` only when the model carries no recognizable vendor
/// signature at all (honest, never fabricated); a recognized vendor with an
/// unrecognized specific id still gets a same-tier approximate estimate rather
/// than vanishing (see `pricing::price_for_model`'s fallback policy).
pub(crate) fn est_cost_usd(model: &Option<String>, exact: &ExactComposition) -> Option<f64> {
    let model = model.as_deref()?;
    // Resident context decides which long-context price tier applies, so it must
    // count cache writes too: they occupy the window exactly like the rest.
    let lookup = pricing::price_for_model(model, exact.resident())?;
    let rate = lookup.pricing;
    let cache_read_rate = rate.input * pricing::CACHE_READ_MULTIPLIER;
    // Writing a token into the cache costs MORE than sending it fresh, not less.
    // The 5 minute TTL is the default, so that is the multiplier billed here.
    let cache_write_rate = rate.input * pricing::CACHE_WRITE_MULTIPLIER_5M;

    let cost = (exact.cache_read as f64 / 1_000_000.0) * cache_read_rate
        + (exact.fresh as f64 / 1_000_000.0) * rate.input
        + (exact.cache_write as f64 / 1_000_000.0) * cache_write_rate
        + (exact.output as f64 / 1_000_000.0) * rate.output;
    Some(cost)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(cache_read: u64, fresh: u64, output: u64) -> ExactComposition {
        ExactComposition { cache_read, fresh, cache_write: 0, output }
    }

    /// Cache WRITES bill at a premium over fresh input, not at parity with it.
    /// This is the regression guard for the gap that made the modelled premium
    /// unreachable: the split used to be summed into `fresh` before it ever got
    /// here, so a price table that knew the multiplier could never apply it.
    #[test]
    fn cache_writes_bill_above_fresh_input() {
        let model = Some("claude-opus-4-8".to_string());
        let one_m = 1_000_000u64;

        let fresh_only = ExactComposition { cache_read: 0, fresh: one_m, cache_write: 0, output: 0 };
        let write_only = ExactComposition { cache_read: 0, fresh: 0, cache_write: one_m, output: 0 };

        let fresh_cost = est_cost_usd(&model, &fresh_only).expect("opus -> a cost");
        let write_cost = est_cost_usd(&model, &write_only).expect("opus -> a cost");

        assert!(
            (fresh_cost - 5.0).abs() < 1e-6,
            "1M fresh tokens bill at the input rate ($5.00), got {fresh_cost}"
        );
        assert!(
            (write_cost / fresh_cost - pricing::CACHE_WRITE_MULTIPLIER_5M).abs() < 1e-6,
            "cache writes must bill at exactly {}x input, got {}",
            pricing::CACHE_WRITE_MULTIPLIER_5M,
            write_cost / fresh_cost
        );
    }

    /// The corrected Opus rate ($5.00 / $25.00, not the retired $15.00 / $75.00
    /// the old table used for every "opus" match): cache reads bill at 0.1x
    /// input ($0.50 for 1M cache-read tokens), fresh input at the full input
    /// rate ($5.00 for 1M fresh tokens), and cache reads stay strictly cheaper
    /// than the same volume of fresh input.
    #[test]
    fn est_cost_bills_the_corrected_opus_5_rate() {
        let model = Some("claude-opus-4-8".to_string());

        let cache_only = usage(1_000_000, 0, 0);
        let cost = est_cost_usd(&model, &cache_only).expect("opus is priced");
        assert!((cost - 0.50).abs() < 1e-6, "1M cache-read tokens at the corrected $5 input rate, got {cost}");

        let fresh_only = usage(0, 1_000_000, 0);
        let fresh_cost = est_cost_usd(&model, &fresh_only).expect("opus is priced");
        assert!((fresh_cost - 5.0).abs() < 1e-6, "1M fresh tokens at the corrected $5 input rate, got {fresh_cost}");

        assert!(cost < fresh_cost, "cache reads must stay cheaper than fresh input");
    }

    /// Defect: claude-fable-5 used to return NO cost estimate at all. It must
    /// now price like any other known model.
    #[test]
    fn est_cost_prices_fable_5() {
        let model = Some("claude-fable-5".to_string());
        let exact = usage(0, 1_000_000, 0);
        let cost = est_cost_usd(&model, &exact).expect("fable 5 must be priced");
        assert!((cost - 10.0).abs() < 1e-6, "1M fresh tokens at the $10 input rate, got {cost}");
    }

    /// A model with no recognizable vendor signature at all stays nullable, and
    /// a session with no model at all stays nullable too.
    #[test]
    fn est_cost_is_none_for_a_truly_unknown_model_or_no_model() {
        let exact = usage(1_000_000, 0, 0);
        assert_eq!(est_cost_usd(&Some("mystery".into()), &exact), None);
        assert_eq!(est_cost_usd(&None, &exact), None);
    }
}
