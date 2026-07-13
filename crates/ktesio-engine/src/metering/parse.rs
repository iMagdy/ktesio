//! The OpenAI-compatible `usage` PARSE (spine AD-7 v1 `engine-observed` half),
//! story 3-4 — pure, cross-OS by construction (NO OS cfg).
//!
//! The engine-observed listener ([`super::listener`]) forwards the agent's model
//! traffic to the real upstream and, on a completion response, skims the standard
//! OpenAI-compatible `usage` object out of the (non-streaming) JSON body:
//!
//! ```json
//! {"choices": [ ... ], "usage": {"prompt_tokens": 128, "completion_tokens": 512}}
//! ```
//!
//! The two fields map onto the SAME [`ParsedUsage`](crate::ports::ParsedUsage)
//! shape the self-reported channel yields (AD-14 snake_case columns):
//! `prompt_tokens → input_tokens`, `completion_tokens → output_tokens`. The
//! ENGINE mints the per-Run `sequence` (the observed agent supplies none — see
//! [`super::ObservedListener`]), so this function returns ONLY the token counts.
//!
//! ## Robustness (best-effort to the RUN — mirrors 3-1's malformed-line skip)
//!
//! A body with NO `usage` object, a malformed/partial JSON body, or a `usage`
//! object missing a field yields `None` — the observation is SKIPPED (a
//! diagnostic, never a panic, never on `kt` stdout). Observation is best-effort:
//! the agent's call still succeeds (the listener relays the response faithfully
//! regardless), so a parse miss loses one measurement, never the call.
//!
//! ## Streaming (documented v1 deferral)
//!
//! An OpenAI STREAMING response (`stream: true`) emits `usage` only in a final SSE
//! `data:` chunk, and only when `stream_options.include_usage` is set — a
//! different wire shape (Server-Sent Events, not one JSON body). v1 parses the
//! NON-STREAMING JSON body only; streamed-usage parsing is a documented deferral
//! (recorded in `docs/architecture.md` + the story), so a streamed response is
//! simply not metered rather than silently mis-parsed. This is bounded by AD-7's
//! `[ASSUMPTION: OpenAI-compatible usage JSON covers v1 targets]`.

/// The parsed OpenAI-compatible `usage` counts skimmed from a completion response
/// body: `(input_tokens, output_tokens)`, mapped from `prompt_tokens` /
/// `completion_tokens`. See the module docs.
///
/// Returns `None` when the body is not a JSON object with a well-formed `usage`
/// object carrying both integer fields (a missing/partial/malformed body, or a
/// streaming SSE body) — the caller SKIPS it (best-effort, never a panic). Both
/// counts are read as non-negative `u64` (a negative or non-integer field → the
/// whole parse is `None`, since a negative token count is nonsense).
///
/// PURE — no I/O, no OS cfg. Tolerates extra fields (the real response carries
/// `choices`, `model`, `id`, and often `usage` sub-fields like
/// `total_tokens`/`prompt_tokens_details` we ignore): only the two counts are
/// read, so a provider adding fields never breaks the parse.
pub fn parse_openai_usage(body: &[u8]) -> Option<(u64, u64)> {
    // Parse leniently into a serde_json::Value: the real response is a large object
    // with many fields we do not model, so a typed struct with deny_unknown_fields
    // would reject legitimate responses. We read exactly the two counts we need.
    let value: serde_json::Value = serde_json::from_slice(body).ok()?;
    let usage = value.get("usage")?;
    let input = usage_field(usage, "prompt_tokens")?;
    let output = usage_field(usage, "completion_tokens")?;
    Some((input, output))
}

/// Read a single non-negative integer token field from the `usage` object.
///
/// Accepts a JSON integer (the OpenAI form). A field that is absent, a non-integer
/// (float/string/null), or negative yields `None` — which makes the whole
/// [`parse_openai_usage`] return `None` (skip), never a partial or wrapped count.
fn usage_field(usage: &serde_json::Value, key: &str) -> Option<u64> {
    let field = usage.get(key)?;
    // as_u64 accepts a non-negative JSON integer; a float / negative / string is
    // None (a token count is a non-negative whole number — anything else is
    // malformed and the measurement is skipped).
    field.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_well_formed_completion_body() {
        // AC-A / AC5: a non-streaming completion response with a standard `usage`
        // object yields the mapped (input, output) counts.
        let body = br#"{
            "id": "chatcmpl-abc",
            "object": "chat.completion",
            "model": "gpt-4o",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}}],
            "usage": {"prompt_tokens": 128, "completion_tokens": 512, "total_tokens": 640}
        }"#;
        assert_eq!(parse_openai_usage(body), Some((128, 512)));
    }

    #[test]
    fn maps_prompt_to_input_and_completion_to_output() {
        // The field-name mapping is the crux: prompt_tokens → input_tokens,
        // completion_tokens → output_tokens (NOT swapped).
        let body = br#"{"usage": {"prompt_tokens": 7, "completion_tokens": 3}}"#;
        let (input, output) = parse_openai_usage(body).expect("well-formed");
        assert_eq!(input, 7, "prompt_tokens maps to input");
        assert_eq!(output, 3, "completion_tokens maps to output");
    }

    #[test]
    fn tolerates_extra_and_unknown_fields() {
        // A real response carries many fields (system_fingerprint, usage sub-detail
        // objects, service_tier); the two counts still parse (no deny_unknown_fields).
        let body = br#"{
            "system_fingerprint": "fp_x",
            "service_tier": "default",
            "usage": {
                "prompt_tokens": 11,
                "completion_tokens": 22,
                "total_tokens": 33,
                "prompt_tokens_details": {"cached_tokens": 0},
                "completion_tokens_details": {"reasoning_tokens": 4}
            }
        }"#;
        assert_eq!(parse_openai_usage(body), Some((11, 22)));
    }

    #[test]
    fn a_body_with_no_usage_object_is_skipped() {
        // A response WITHOUT `usage` (some endpoints, or an error body) → None
        // (skipped, no panic). Observation is best-effort — the call still succeeded.
        let body = br#"{"id": "x", "choices": [{"index": 0}]}"#;
        assert_eq!(parse_openai_usage(body), None);
    }

    #[test]
    fn a_usage_missing_a_field_is_skipped() {
        // A `usage` object missing one of the two counts → None (never a partial
        // count that would half-bill).
        let only_prompt = br#"{"usage": {"prompt_tokens": 5}}"#;
        assert_eq!(parse_openai_usage(only_prompt), None);
        let only_completion = br#"{"usage": {"completion_tokens": 5}}"#;
        assert_eq!(parse_openai_usage(only_completion), None);
    }

    #[test]
    fn a_malformed_or_partial_body_is_skipped_not_panicked() {
        // A truncated / non-JSON / empty body → None, never a panic (a streamed
        // chunk, a network-truncated body, or a plain-text error page all land here).
        for bad in [
            &b"not json at all"[..],
            &b"{\"usage\": {\"prompt_tokens\": 1,"[..], // truncated
            &b""[..],                                   // empty
            &b"data: {\"usage\":{}}\n\n"[..],           // an SSE stream chunk (deferred)
        ] {
            assert_eq!(parse_openai_usage(bad), None, "must skip: {bad:?}");
        }
    }

    #[test]
    fn a_negative_or_non_integer_count_is_skipped() {
        // A negative or float token count is nonsense (a token count is a
        // non-negative whole number) → None, never a wrapped/truncated value.
        let negative = br#"{"usage": {"prompt_tokens": -1, "completion_tokens": 2}}"#;
        assert_eq!(parse_openai_usage(negative), None);
        let float = br#"{"usage": {"prompt_tokens": 1.5, "completion_tokens": 2}}"#;
        assert_eq!(parse_openai_usage(float), None);
        let string = br#"{"usage": {"prompt_tokens": "10", "completion_tokens": 2}}"#;
        assert_eq!(parse_openai_usage(string), None);
    }

    #[test]
    fn zero_counts_are_valid() {
        // A legitimate zero-token event (an empty completion) parses as (0, 0) —
        // distinct from a missing field (None).
        let body = br#"{"usage": {"prompt_tokens": 0, "completion_tokens": 0}}"#;
        assert_eq!(parse_openai_usage(body), Some((0, 0)));
    }

    #[test]
    fn a_large_count_within_u64_parses() {
        // A large but valid count parses (the storage-boundary clamp to i64 is the
        // ledger's job, story 3-1 — here we just read the u64 faithfully).
        let body = br#"{"usage": {"prompt_tokens": 9007199254740993, "completion_tokens": 1}}"#;
        assert_eq!(parse_openai_usage(body), Some((9_007_199_254_740_993, 1)));
    }
}
