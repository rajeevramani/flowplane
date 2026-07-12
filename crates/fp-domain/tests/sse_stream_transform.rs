//! Spec-first integration tests for incremental SSE stream transformation
//! (bead fpv2-o6w.1, design "AI gateway incremental SSE streaming").
//!
//! These tests are written purely from the public-API specification of
//! `complete_sse_events_end` and `strip_synthetic_openai_usage_sse`. They do not
//! reference implementation internals.

use fp_domain::{complete_sse_events_end, strip_synthetic_openai_usage_sse, OpenAiTokenUsage};

// ---------------------------------------------------------------------------
// Canonical OpenAI-style SSE event payloads
// ---------------------------------------------------------------------------

/// Content-delta chunk. Contains multi-byte UTF-8 ("héllo ✓ 日本語") so that
/// byte-boundary splits land inside multi-byte sequences.
const DELTA_JSON: &str = r#"{"id":"chatcmpl-42","object":"chat.completion.chunk","model":"gpt-x","choices":[{"index":0,"delta":{"content":"héllo ✓ 日本語"},"finish_reason":null}]}"#;

/// Usage-only chunk as injected via `stream_options: {"include_usage": true}`.
const USAGE_JSON: &str = r#"{"id":"chatcmpl-42","object":"chat.completion.chunk","model":"gpt-x","choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}}"#;

fn expected_usage() -> OpenAiTokenUsage {
    OpenAiTokenUsage {
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
    }
}

/// Build one SSE event: `data: <payload><delimiter>`.
fn event(payload: &str, delimiter: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(payload.as_bytes());
    out.extend_from_slice(delimiter.as_bytes());
    out
}

/// Canonical three-event stream: content delta, usage-only, `[DONE]`, with the
/// given per-event delimiters.
fn canonical_stream(d1: &str, d2: &str, d3: &str) -> Vec<u8> {
    let mut s = Vec::new();
    s.extend_from_slice(&event(DELTA_JSON, d1));
    s.extend_from_slice(&event(USAGE_JSON, d2));
    s.extend_from_slice(&event("[DONE]", d3));
    s
}

// ---------------------------------------------------------------------------
// Streaming-loop simulation harness
// ---------------------------------------------------------------------------

/// Simulate the incremental streaming loop over `chunks`:
/// accumulate into a remainder buffer, take the complete-events prefix
/// (eos=true only on the final chunk), strip it, forward, drain.
/// Returns (client_buffer, usages_observed, final_remainder).
fn simulate(
    chunks: &[&[u8]],
    include_usage_injected: bool,
) -> (Vec<u8>, Vec<OpenAiTokenUsage>, Vec<u8>) {
    let mut remainder: Vec<u8> = Vec::new();
    let mut client: Vec<u8> = Vec::new();
    let mut usages: Vec<OpenAiTokenUsage> = Vec::new();
    let last = chunks.len().saturating_sub(1);
    for (i, chunk) in chunks.iter().enumerate() {
        remainder.extend_from_slice(chunk);
        let eos = i == last;
        let end = complete_sse_events_end(&remainder, eos);
        assert!(
            end <= remainder.len(),
            "complete_sse_events_end returned {end} > buffer len {} (chunk {i}, eos={eos})",
            remainder.len()
        );
        if eos {
            assert_eq!(
                end,
                remainder.len(),
                "end_of_stream=true must flush the whole buffer (chunk {i})"
            );
        }
        let (forwarded, usage) =
            strip_synthetic_openai_usage_sse(&remainder[..end], include_usage_injected);
        client.extend_from_slice(&forwarded);
        if let Some(u) = usage {
            usages.push(u);
        }
        remainder.drain(..end);
    }
    (client, usages, remainder)
}

/// Message-rich byte-equality assertion.
fn assert_bytes_eq(actual: &[u8], expected: &[u8], ctx: &str) {
    assert!(
        actual == expected,
        "{ctx}: forwarded bytes differ\n  actual   ({} bytes): {:?}\n  expected ({} bytes): {:?}",
        actual.len(),
        String::from_utf8_lossy(actual),
        expected.len(),
        String::from_utf8_lossy(expected),
    );
}

// ---------------------------------------------------------------------------
// 1. Byte-boundary chunking safety (every split, three framings)
// ---------------------------------------------------------------------------

/// Split `stream` at every byte boundary into two chunks and assert the
/// streaming loop matches the single-call whole-stream result, usage is
/// observed exactly once, and the remainder is empty at end of stream.
fn assert_every_split_safe(stream: &[u8], framing: &str, expected_client: &[u8]) {
    // Whole-stream reference: eos flushes everything.
    let whole_end = complete_sse_events_end(stream, true);
    assert_eq!(
        whole_end,
        stream.len(),
        "[{framing}] end_of_stream=true must return buffer.len()"
    );
    let (reference_client, reference_usage) = strip_synthetic_openai_usage_sse(stream, true);
    assert_bytes_eq(
        &reference_client,
        expected_client,
        &format!("[{framing}] whole-stream strip reference"),
    );
    let reference_usage = reference_usage
        .unwrap_or_else(|| panic!("[{framing}] whole-stream strip must observe usage"));
    assert_eq!(
        reference_usage,
        expected_usage(),
        "[{framing}] whole-stream usage value mismatch"
    );

    for split in 0..=stream.len() {
        let (client, usages, remainder) = simulate(&[&stream[..split], &stream[split..]], true);
        assert!(
            remainder.is_empty(),
            "[{framing}] split {split}: remainder not empty at end of stream: {:?}",
            String::from_utf8_lossy(&remainder)
        );
        assert_eq!(
            usages.len(),
            1,
            "[{framing}] split {split}: usage observed {} times (must be exactly once); \
             observations: {usages:?}",
            usages.len()
        );
        assert_eq!(
            usages[0],
            expected_usage(),
            "[{framing}] split {split}: wrong usage value"
        );
        assert_bytes_eq(
            &client,
            &reference_client,
            &format!("[{framing}] split {split}"),
        );
    }
}

#[test]
fn chunking_every_boundary_lf() {
    let stream = canonical_stream("\n\n", "\n\n", "\n\n");
    let mut expected = event(DELTA_JSON, "\n\n");
    expected.extend_from_slice(&event("[DONE]", "\n\n"));
    assert_every_split_safe(&stream, "LF", &expected);
}

#[test]
fn chunking_every_boundary_crlf() {
    let stream = canonical_stream("\r\n\r\n", "\r\n\r\n", "\r\n\r\n");
    let mut expected = event(DELTA_JSON, "\r\n\r\n");
    expected.extend_from_slice(&event("[DONE]", "\r\n\r\n"));
    assert_every_split_safe(&stream, "CRLF", &expected);
}

#[test]
fn chunking_every_boundary_mixed() {
    // Delta LF-framed, usage CRLF-framed, [DONE] LF-framed. Original delimiters
    // of kept events must survive untouched (no line-ending normalization).
    let stream = canonical_stream("\n\n", "\r\n\r\n", "\n\n");
    let mut expected = event(DELTA_JSON, "\n\n");
    expected.extend_from_slice(&event("[DONE]", "\n\n"));
    assert_every_split_safe(&stream, "mixed", &expected);
}

// ---------------------------------------------------------------------------
// 2. Adversarial inputs
// ---------------------------------------------------------------------------

#[test]
fn usage_json_split_mid_token_still_stripped_once() {
    let stream = canonical_stream("\n\n", "\n\n", "\n\n");
    let pos = stream
        .windows(b"total_tokens".len())
        .position(|w| w == b"total_tokens")
        .expect("stream must contain the total_tokens key");
    // Split in the middle of the `total_tokens` JSON token.
    let split = pos + 6;
    let (reference_client, _) = strip_synthetic_openai_usage_sse(&stream, true);

    let (client, usages, remainder) = simulate(&[&stream[..split], &stream[split..]], true);
    assert!(
        remainder.is_empty(),
        "mid-token split {split}: remainder not empty: {:?}",
        String::from_utf8_lossy(&remainder)
    );
    assert_eq!(
        usages.len(),
        1,
        "mid-token split {split}: usage observed {} times (must be exactly once)",
        usages.len()
    );
    assert_eq!(
        usages[0],
        expected_usage(),
        "mid-token split {split}: wrong usage value"
    );
    assert_bytes_eq(
        &client,
        &reference_client,
        &format!("mid-token split {split}"),
    );
}

#[test]
fn crlf_delimiter_split_after_trailing_cr() {
    // CRLF-framed stream, split exactly as `...\r\n\r` + `\n...` on the FIRST
    // delimiter: the trailing lone \r must be deferred, with no byte loss and
    // no duplication.
    let stream = canonical_stream("\r\n\r\n", "\r\n\r\n", "\r\n\r\n");
    let delim_pos = stream
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("CRLF stream must contain a \\r\\n\\r\\n delimiter");
    let split = delim_pos + 3; // first chunk ends with ...\r\n\r

    // Passthrough mode: output must be byte-identical to the input.
    let (client, usages, remainder) = simulate(&[&stream[..split], &stream[split..]], false);
    assert!(
        remainder.is_empty(),
        "trailing-CR split {split} (passthrough): remainder not empty: {:?}",
        String::from_utf8_lossy(&remainder)
    );
    assert_bytes_eq(
        &client,
        &stream,
        &format!("trailing-CR split {split} (passthrough)"),
    );
    assert_eq!(
        usages,
        vec![expected_usage()],
        "trailing-CR split {split} (passthrough): usage must still be observed exactly once"
    );

    // Strip mode: must match the whole-stream single-call result.
    let (reference_client, _) = strip_synthetic_openai_usage_sse(&stream, true);
    let (client, usages, remainder) = simulate(&[&stream[..split], &stream[split..]], true);
    assert!(
        remainder.is_empty(),
        "trailing-CR split {split} (strip): remainder not empty: {:?}",
        String::from_utf8_lossy(&remainder)
    );
    assert_eq!(
        usages,
        vec![expected_usage()],
        "trailing-CR split {split} (strip): usage must be observed exactly once"
    );
    assert_bytes_eq(
        &client,
        &reference_client,
        &format!("trailing-CR split {split} (strip)"),
    );
}

#[test]
fn passthrough_identity_when_not_injected() {
    // include_usage_injected == false: EVERY byte passes through untouched,
    // even though the stream contains a usage event; usage is still observed.
    for (framing, delim) in [("LF", "\n\n"), ("CRLF", "\r\n\r\n")] {
        let stream = canonical_stream(delim, delim, delim);
        let (forwarded, usage) = strip_synthetic_openai_usage_sse(&stream, false);
        assert_bytes_eq(
            &forwarded,
            &stream,
            &format!("[{framing}] passthrough identity"),
        );
        let usage = usage.unwrap_or_else(|| {
            panic!("[{framing}] passthrough must still observe the usage event")
        });
        assert_eq!(
            usage,
            expected_usage(),
            "[{framing}] passthrough usage value mismatch"
        );
    }
}

#[test]
fn stream_without_usage_left_untouched() {
    // No usage event at all: nothing stripped, usage None — for BOTH modes.
    let mut stream = event(DELTA_JSON, "\n\n");
    stream.extend_from_slice(&event("[DONE]", "\n\n"));

    for injected in [true, false] {
        let (forwarded, usage) = strip_synthetic_openai_usage_sse(&stream, injected);
        assert_bytes_eq(
            &forwarded,
            &stream,
            &format!("no-usage stream (injected={injected})"),
        );
        assert!(
            usage.is_none(),
            "no-usage stream (injected={injected}): usage must be None, got {usage:?}"
        );
    }
}

#[test]
fn invalid_utf8_event_kept_verbatim() {
    // An event containing invalid UTF-8 between valid events must be kept
    // verbatim, and stripping of the surrounding usage event must still work.
    let invalid_event: &[u8] = b"data: \xFF\xFE\xFA garbage\n\n";
    let mut stream = event(DELTA_JSON, "\n\n");
    stream.extend_from_slice(invalid_event);
    stream.extend_from_slice(&event(USAGE_JSON, "\n\n"));
    stream.extend_from_slice(&event("[DONE]", "\n\n"));

    let mut expected = event(DELTA_JSON, "\n\n");
    expected.extend_from_slice(invalid_event);
    expected.extend_from_slice(&event("[DONE]", "\n\n"));

    let (forwarded, usage) = strip_synthetic_openai_usage_sse(&stream, true);
    assert_bytes_eq(
        &forwarded,
        &expected,
        "invalid-UTF-8 event stream (injected=true)",
    );
    let usage =
        usage.expect("usage event surrounded by invalid-UTF-8 event must still be observed");
    assert_eq!(
        usage,
        expected_usage(),
        "invalid-UTF-8 stream: wrong usage value"
    );
}

#[test]
fn consecutive_blank_lines_byte_identity_passthrough() {
    // Byte-identity is the contract when include_usage_injected == false; we
    // make no assumption about how empty events are segmented.
    let stream: &[u8] = b"data: a\n\n\n\ndata: b\n\n";

    // Whole-stream single call.
    let end = complete_sse_events_end(stream, true);
    assert_eq!(
        end,
        stream.len(),
        "eos must flush the whole consecutive-blank-line buffer"
    );
    let (forwarded, usage) = strip_synthetic_openai_usage_sse(stream, false);
    assert_bytes_eq(
        &forwarded,
        stream,
        "consecutive blank lines (whole stream, passthrough)",
    );
    assert!(usage.is_none(), "no usage event present, got {usage:?}");

    // Every two-chunk split must also preserve byte identity.
    for split in 0..=stream.len() {
        let (client, usages, remainder) = simulate(&[&stream[..split], &stream[split..]], false);
        assert!(
            remainder.is_empty(),
            "consecutive blank lines split {split}: remainder not empty: {:?}",
            String::from_utf8_lossy(&remainder)
        );
        assert!(
            usages.is_empty(),
            "consecutive blank lines split {split}: unexpected usage {usages:?}"
        );
        assert_bytes_eq(
            &client,
            stream,
            &format!("consecutive blank lines split {split} (passthrough)"),
        );
    }
}

#[test]
fn done_event_kept_when_injected() {
    let stream: &[u8] = b"data: [DONE]\n\n";
    let (forwarded, usage) = strip_synthetic_openai_usage_sse(stream, true);
    assert_bytes_eq(
        &forwarded,
        stream,
        "data: [DONE] must be kept when injected=true",
    );
    assert!(
        usage.is_none(),
        "[DONE] event must not report usage, got {usage:?}"
    );
}

#[test]
fn empty_buffer_contract() {
    assert_eq!(
        complete_sse_events_end(b"", false),
        0,
        "empty buffer, eos=false must return 0"
    );
    assert_eq!(
        complete_sse_events_end(b"", true),
        0,
        "empty buffer, eos=true must return buffer.len() == 0"
    );
    for injected in [true, false] {
        let (forwarded, usage) = strip_synthetic_openai_usage_sse(b"", injected);
        assert!(
            forwarded.is_empty(),
            "strip of empty buffer (injected={injected}) must forward nothing, got {forwarded:?}"
        );
        assert!(
            usage.is_none(),
            "strip of empty buffer (injected={injected}) must observe no usage, got {usage:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 3. complete_sse_events_end contract table
// ---------------------------------------------------------------------------

#[test]
fn complete_sse_events_end_contract_table() {
    // No blank line yet -> 0.
    assert_eq!(
        complete_sse_events_end(b"data: x", false),
        0,
        "partial event with no blank line must return 0"
    );
    assert_eq!(
        complete_sse_events_end(b"data: x\n", false),
        0,
        "single line terminator is not a blank line yet"
    );

    // Exactly one complete LF event -> its full length.
    assert_eq!(
        complete_sse_events_end(b"data: x\n\n", false),
        9,
        "one complete LF event must return its full length (9)"
    );

    // Complete CRLF event -> full length.
    assert_eq!(
        complete_sse_events_end(b"data: x\r\n\r\n", false),
        11,
        "one complete CRLF event must return its full length (11)"
    );

    // Complete event + partial next -> length of the first event only.
    assert_eq!(
        complete_sse_events_end(b"data: x\n\ndata: y", false),
        9,
        "complete event + partial next must return length of the first event only"
    );

    // Trailing lone \r is deferred: `...\r\n\r` — its event is NOT complete
    // (a following \n could extend the \r to \r\n).
    assert_eq!(
        complete_sse_events_end(b"data: x\r\n\r", false),
        0,
        "trailing ...\\r\\n\\r must be deferred (event not complete)"
    );
    assert_eq!(
        complete_sse_events_end(b"data: x\n\ndata: y\r\n\r", false),
        9,
        "first event complete; second with trailing ...\\r\\n\\r deferred"
    );

    // end_of_stream == true always returns buffer.len().
    for buf in [
        &b"data: x"[..],
        &b"data: x\n\n"[..],
        &b"data: x\r\n\r"[..],
        &b"data: x\n\ndata: y"[..],
    ] {
        assert_eq!(
            complete_sse_events_end(buf, true),
            buf.len(),
            "end_of_stream=true must return buffer.len() for {:?}",
            String::from_utf8_lossy(buf)
        );
    }
}
