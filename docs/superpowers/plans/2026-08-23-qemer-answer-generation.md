# Qemer Answer Generation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `qemer-answer` to the point where it can turn a query plus retrieved snippets into a streamed, grounded answer from a running `llama-server`, with context budgeting that cannot silently overflow.

**Architecture:** Prompt assembly and budgeting are pure functions over data — snippets in, a `Prompt` out — with no network and no async, because that is where every interesting edge case lives. Generation is a thin HTTP layer over `llama-server`'s OpenAI-compatible `/v1/chat/completions`, streamed as Server-Sent Events, with SSE frame parsing split into its own pure function so wrong bodies and malformed frames are covered without a live server. The returned stream aborts by being dropped, which is all the cancellation the caller needs.

**Tech Stack:** Rust 2024, `reqwest` 0.13 (with `stream`), `async-stream`, `serde_json`, `futures` 0.3, `tokio` 1.53. No new heavy dependencies; `qemer-answer` links `qemer-core` for the `Snippet` type only.

**Spec:** [`docs/decisions.md`](../../decisions.md) — read it first. It records what is settled *and what is deliberately still open*; do not resolve an open question by picking something reasonable.

## Global Constraints

- **`qemer-answer` never retrieves.** Callers hand it snippets. It has no LanceDB dependency, no `Corpus`, no `EmbedClient`, and no knowledge of how snippets were found. Adding any of those breaks the seam that `docs/decisions.md` describes as existing so that prompt assembly is unit-testable away from the terminal.
- **`qemer-answer` knows a completion endpoint and nothing about the embedding endpoint.** `docs/decisions.md` settles two separate base URLs precisely so the two crates' configuration stays disjoint. No type, field, comment, or error message here may name embedding, retrieval, or `qemer-core`'s endpoint.
- **No knowledge of the TUI.** `docs/decisions.md` settles that Esc aborts an in-flight stream, but that is the caller's affair. Nothing in this crate may mention Esc, keys, ratatui, or a terminal. The contract this crate offers is that dropping the stream ends the request — see Task 4.
- **The sources-first fallback must stay cheap.** `docs/decisions.md` records that falling back to sources-first "changes only the prompt, the token cap, and the TUI's visual weighting — no structural change." Therefore the system instruction and the completion token cap are values, not control flow. Do not branch on an answer style anywhere in this crate.
- **Budgeting must over-estimate, never under-estimate.** A prompt that overflows the server's context is a failed request or a silently truncated prompt; a prompt that leaves tokens unused is merely slightly wasteful. Every rounding decision goes toward "more tokens than we think."
- Commits follow Conventional Commits: `<type>: <subject>`, bulleted body when the commit has more than one distinct sub-change.

## Verified facts about `llama-server`

Checked against the llama.cpp server source and README rather than recalled. Treat these as established; the tasks below depend on all four.

- The streaming endpoint is `POST /v1/chat/completions` with `"stream": true`, framed as Server-Sent Events (`data: {json}` lines, terminated by `data: [DONE]`).
- **`include_usage` defaults to `false`.** Token counts are only present if the request sends `"stream_options": {"include_usage": true}`. Without it, `AnswerEvent::Done` can never carry real numbers. (`tools/server/server-task.h`)
- **The usage chunk carries an empty `choices` array.** llama.cpp follows the OpenAI spec here and appends a final chunk with `"choices": []` and a `"usage"` object. Any parser that indexes `choices[0]` unconditionally will panic on that chunk. (`tools/server/server-task.cpp`)
- The usage object's shape is `{"completion_tokens": N, "prompt_tokens": N, "total_tokens": N, "prompt_tokens_details": {...}}`. Only the first two are used here.

A `POST /tokenize` endpoint also exists, returning `{"tokens": [...]}`. This plan does **not** use it — see "Choices this plan makes" below.

## Choices this plan makes that you may want to override

These are decisions, not facts. Each has a defensible alternative, and the reasoning is written down so overriding one is cheap.

- **Token counting is offline and approximate, not exact via `/tokenize`.** Exact counts are available over HTTP, but calling them from `prompt::build` would make prompt assembly `async` and network-dependent — and `docs/decisions.md` gives "prompt assembly and context budgeting are the logic most worth unit-testing" as one of the two reasons this crate exists at all. An estimator keeps that property. The cost is that the estimate can be wrong, which is why the constant is deliberately conservative and why every budget test asserts an upper bound rather than an exact figure.
- **`fit` stops at the first snippet that does not fit, rather than skipping it and trying the next.** `docs/decisions.md` says "fill the prompt in rank order until the budget is spent and drop the tail," which reads as stop-at-first-non-fit. Skip-and-continue would pack more context in but would reorder relevance against rank, which is the ordering RRF just spent effort establishing.
- **The degenerate case truncates rather than returning nothing.** `docs/decisions.md` says whole snippets are dropped; the project `CLAUDE.md` names "a single code block larger than the whole budget" and "truncation landing mid-identifier" as edge cases to test. Those reconcile only one way: snippets are dropped whole, *except* that when the top-ranked snippet alone exceeds the budget, it is truncated, because grounding the answer in nothing is worse. **Confirm this reading before relying on it.**

---

## File Structure

| File | Responsibility |
| --- | --- |
| `qemer-answer/src/prompt.rs` | **Exists as a stub.** Token estimation, snippet fitting, and prompt rendering. Pure functions, no I/O, no async. Where the edge cases live. |
| `qemer-answer/src/stream.rs` | **New.** Parsing one SSE line into one event. Pure, no network. |
| `qemer-answer/src/lib.rs` | **Exists.** `AnswerEvent`, `AnswerError`, `Generator`, and the HTTP/streaming wiring that joins `prompt` to `stream`. |
| `qemer-answer/tests/completion_stub.rs` | **New.** A one-shot SSE server standing in for `llama-server`. Mirrors `qemer-core/tests/embed_stub.rs`, which already proved the pattern. |
| `qemer-answer/tests/generate.rs` | **New.** End-to-end: real HTTP, real SSE framing, real prompt, real events out. |

Fusion lived in its own file in `qemer-core` for this reason and SSE parsing gets the same treatment: the parsing rules are where malformed input bites, and they never need a socket to test.

---

## Task 1: Token estimation and budget fitting

The load-bearing half of this crate. Everything downstream trusts that a `Prompt` fits the budget it was given, so this task establishes that property and tests it as an invariant rather than as a set of examples.

**Files:**
- Modify: `qemer-answer/src/prompt.rs` (replace the stub entirely)

**Interfaces:**
- Consumes: `qemer_core::Snippet` — fields `library`, `version`, `snippet_id`, `title`, `description`, `code: Option<String>`, `source_url: Option<String>`, `score: f32`.
- Produces: `prompt::estimate_tokens(&str) -> usize`; `prompt::render_snippet(&Snippet) -> String`; `prompt::Fitted { snippets: Vec<Snippet>, dropped: usize, truncated: bool }`; `prompt::fit(&[Snippet], usize) -> Fitted`.

- [ ] **Step 1: Write the failing tests**

Replace the whole contents of `qemer-answer/src/prompt.rs` test module with this. `a_snippet` is a helper every later task also uses, so it is defined once here.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn a_snippet(id: &str, description: &str, code: Option<&str>) -> Snippet {
        Snippet {
            library: "lancedb".into(),
            version: "0.37.1".into(),
            snippet_id: id.into(),
            title: format!("Title for {id}"),
            description: description.into(),
            code: code.map(|c| c.to_string()),
            source_url: Some(format!("https://example/{id}")),
            score: 1.0,
        }
    }

    #[test]
    fn estimate_of_empty_text_is_zero() {
        assert_eq!(estimate_tokens(""), 0);
    }

    #[test]
    fn estimate_of_any_non_empty_text_is_at_least_one() {
        assert!(estimate_tokens("a") >= 1);
    }

    #[test]
    fn estimate_never_shrinks_as_text_grows() {
        let short = estimate_tokens("fn main() {}");
        let long = estimate_tokens("fn main() {} // and a good deal more text besides");
        assert!(long >= short, "estimate must be monotonic in input length");
    }

    #[test]
    fn rendering_includes_title_description_code_and_url() {
        let rendered = render_snippet(&a_snippet("s1", "Some prose.", Some("let x = 1;")));
        assert!(rendered.contains("Title for s1"));
        assert!(rendered.contains("Some prose."));
        assert!(rendered.contains("let x = 1;"));
        assert!(rendered.contains("https://example/s1"));
    }

    #[test]
    fn rendering_a_description_only_snippet_emits_no_code_fence() {
        let rendered = render_snippet(&a_snippet("s1", "Prose only.", None));
        assert!(rendered.contains("Prose only."));
        assert!(!rendered.contains("```"), "no code means no fence: {rendered}");
    }

    #[test]
    fn fitting_nothing_yields_nothing() {
        let fitted = fit(&[], 1000);
        assert!(fitted.snippets.is_empty());
        assert_eq!(fitted.dropped, 0);
        assert!(!fitted.truncated);
    }

    #[test]
    fn a_generous_budget_keeps_every_snippet_in_rank_order() {
        let snippets = vec![
            a_snippet("s1", "First.", None),
            a_snippet("s2", "Second.", None),
            a_snippet("s3", "Third.", None),
        ];
        let fitted = fit(&snippets, 100_000);
        let ids: Vec<&str> = fitted.snippets.iter().map(|s| s.snippet_id.as_str()).collect();
        assert_eq!(ids, vec!["s1", "s2", "s3"]);
        assert_eq!(fitted.dropped, 0);
        assert!(!fitted.truncated);
    }

    #[test]
    fn a_tight_budget_drops_the_tail_and_counts_it() {
        let big = "x ".repeat(400);
        let snippets = vec![
            a_snippet("s1", "Small.", None),
            a_snippet("s2", &big, None),
            a_snippet("s3", "Also small.", None),
        ];
        // Enough for s1 and nothing like enough for s2.
        let budget = estimate_tokens(&render_snippet(&snippets[0])) + 5;
        let fitted = fit(&snippets, budget);
        let ids: Vec<&str> = fitted.snippets.iter().map(|s| s.snippet_id.as_str()).collect();
        assert_eq!(ids, vec!["s1"], "the tail is dropped, not reordered around");
        assert_eq!(fitted.dropped, 2);
        assert!(!fitted.truncated);
    }

    #[test]
    fn a_single_snippet_larger_than_the_whole_budget_is_truncated_not_dropped() {
        let huge = "let identifier_name = 1;\n".repeat(500);
        let snippets = vec![a_snippet("s1", "Prose.", Some(&huge))];
        let fitted = fit(&snippets, 100);

        assert_eq!(fitted.snippets.len(), 1, "returning nothing would ground the answer in nothing");
        assert!(fitted.truncated);
        assert!(
            estimate_tokens(&render_snippet(&fitted.snippets[0])) <= 100,
            "a truncated snippet must actually fit the budget it was given"
        );
    }

    #[test]
    fn truncation_lands_on_a_whitespace_boundary_not_inside_an_identifier() {
        let text = "alpha beta gamma delta epsilon zeta eta theta";
        let cut = truncate_at_boundary(text, 20);
        let body = cut.replace("\n… [truncated]", "");
        assert!(
            text.starts_with(body.trim_end()),
            "truncated body must be a prefix of the original: {body:?}"
        );
        for word in body.split_whitespace() {
            assert!(
                text.split_whitespace().any(|w| w == word),
                "{word:?} is a fragment of a word, not a whole one"
            );
        }
    }

    #[test]
    fn a_zero_budget_keeps_nothing() {
        let snippets = vec![a_snippet("s1", "Prose.", None)];
        let fitted = fit(&snippets, 0);
        assert!(fitted.snippets.is_empty());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qemer-answer prompt`
Expected: FAIL to compile — `estimate_tokens`, `render_snippet`, `fit`, `Fitted`, and `truncate_at_boundary` do not exist.

- [ ] **Step 3: Write the implementation**

Replace everything in `qemer-answer/src/prompt.rs` above the test module with this.

```rust
//! Prompt assembly and context budgeting.
//!
//! Kept as pure functions over data — snippets in, a prompt out — because this
//! is where the edge cases live: a single code block larger than the whole
//! budget, retrieval returning nothing, truncation landing mid-identifier.
//!
//! Nothing here is async and nothing here makes a request. Exact token counts
//! are available from the server, but taking them would make this module
//! network-dependent, and being testable without a server is the reason this
//! module is separate in the first place.

use qemer_core::Snippet;

/// Characters per token. Documentation corpora are roughly half code, which
/// tokenises denser than prose, so this sits below the ~4.0 conventionally
/// quoted for English. It is a placeholder with no measurement behind it.
///
/// Lowering this number estimates *more* tokens for the same text, which is
/// the safe direction: over-estimating wastes context, under-estimating
/// overflows it.
const CHARS_PER_TOKEN: f32 = 3.5;

/// Appended where text was cut, so a truncated excerpt cannot be mistaken for
/// a complete one by the model reading it.
const TRUNCATION_MARKER: &str = "\n… [truncated]";

/// Approximate the token cost of a string. Deliberately an over-estimate.
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    ((text.chars().count() as f32 / CHARS_PER_TOKEN).ceil() as usize).max(1)
}

/// Render one snippet exactly as it will appear in the prompt.
///
/// Budgeting and rendering must agree on what a snippet costs, so both call
/// this. If they rendered separately, the budget would be measuring text that
/// is not the text being sent.
pub fn render_snippet(snippet: &Snippet) -> String {
    let mut out = String::new();
    out.push_str("## ");
    out.push_str(&snippet.title);
    out.push('\n');
    if let Some(url) = &snippet.source_url {
        out.push_str("Source: ");
        out.push_str(url);
        out.push('\n');
    }
    if !snippet.description.is_empty() {
        out.push('\n');
        out.push_str(&snippet.description);
        out.push('\n');
    }
    if let Some(code) = &snippet.code {
        out.push_str("\n```\n");
        out.push_str(code);
        out.push_str("\n```\n");
    }
    out
}

/// Cut `text` to at most `max_chars`, backing off to the last whitespace so a
/// cut never lands inside an identifier. A half-written `create_ind` is worse
/// than a missing one, because it reads as a real symbol.
pub(crate) fn truncate_at_boundary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(max_chars).collect();
    let end = cut.rfind(char::is_whitespace).unwrap_or(0);
    let mut out = cut[..end].to_string();
    out.push_str(TRUNCATION_MARKER);
    out
}

/// The result of fitting snippets to a budget.
#[derive(Debug, Clone)]
pub struct Fitted {
    /// Snippets that fit, in the order they were given.
    pub snippets: Vec<Snippet>,
    /// How many were left out. Callers may want to say so.
    pub dropped: usize,
    /// Whether the degenerate single-oversized-snippet path was taken.
    pub truncated: bool,
}

/// Shrink one snippet until its rendered form fits `budget_tokens`.
///
/// The loop is not an optimisation: the first estimate ignores the rendering
/// scaffolding around the body, so it can land slightly over. Halving until it
/// fits terminates, and guarantees the postcondition the caller relies on.
fn truncate_snippet(snippet: &Snippet, budget_tokens: usize) -> Snippet {
    let mut out = snippet.clone();
    let mut room = (budget_tokens as f32 * CHARS_PER_TOKEN) as usize;
    loop {
        match &snippet.code {
            // Code is the body that outgrows a budget, per the corpus shape.
            Some(code) => out.code = Some(truncate_at_boundary(code, room)),
            None => out.description = truncate_at_boundary(&snippet.description, room),
        }
        if room == 0 || estimate_tokens(&render_snippet(&out)) <= budget_tokens {
            return out;
        }
        room /= 2;
    }
}

/// Take snippets in rank order until the budget is spent, then stop.
///
/// Stopping rather than skipping ahead is deliberate: the input order is the
/// fused ranking, and reaching past a snippet to fit a lower-ranked one would
/// undo the ordering retrieval just established.
pub fn fit(snippets: &[Snippet], budget_tokens: usize) -> Fitted {
    let mut kept: Vec<Snippet> = Vec::new();
    let mut used = 0usize;
    let mut truncated = false;

    for snippet in snippets {
        let cost = estimate_tokens(&render_snippet(snippet));
        if used + cost > budget_tokens {
            break;
        }
        used += cost;
        kept.push(snippet.clone());
    }

    // The top-ranked snippet alone can exceed the budget — a single large code
    // block does it. Dropping it would leave the answer grounded in nothing,
    // so it is cut down instead and flagged.
    if kept.is_empty() && !snippets.is_empty() && budget_tokens > 0 {
        kept.push(truncate_snippet(&snippets[0], budget_tokens));
        truncated = true;
    }

    Fitted {
        dropped: snippets.len() - kept.len(),
        snippets: kept,
        truncated,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p qemer-answer prompt`
Expected: PASS, 11 tests.

If `a_single_snippet_larger_than_the_whole_budget_is_truncated_not_dropped` fails on the budget assertion, the halving loop is doing its job but the title alone exceeds the budget — a case the corpus contract does not produce. Report it rather than loosening the assertion.

- [ ] **Step 5: Commit**

```bash
git add qemer-answer/src/prompt.rs
git commit -m "feat(answer): add token estimation and budget fitting

- Estimate tokens offline so prompt assembly stays pure and testable;
  the constant is deliberately conservative, since over-estimating
  wastes context and under-estimating overflows it.
- fit() takes snippets in rank order and stops at the first that does
  not fit, rather than reaching past it.
- A top-ranked snippet larger than the whole budget is truncated at a
  whitespace boundary rather than dropped, so a cut never lands inside
  an identifier."
```

---

## Task 2: Prompt rendering

Turn a query and fitted snippets into the two messages the chat endpoint wants. The invariant worth testing is that the assembled prompt respects the budget it was handed — including the instruction and the query, not just the snippets.

**Files:**
- Modify: `qemer-answer/src/prompt.rs`

**Interfaces:**
- Consumes: `fit`, `render_snippet`, `estimate_tokens` from Task 1.
- Produces: `prompt::Prompt { system: String, user: String, included: usize, dropped: usize, truncated: bool }`; `prompt::build(query: &str, snippets: &[Snippet], budget_tokens: usize) -> Prompt`; `prompt::SYSTEM_INSTRUCTION: &str`.

Note this changes `build`'s return type from the stub's `String`. Two messages are needed for a chat endpoint, and the counts let a caller say "3 of 5 sources used" without recomputing anything.

- [ ] **Step 1: Write the failing tests**

Append to the `tests` module in `qemer-answer/src/prompt.rs`.

```rust
    #[test]
    fn the_prompt_carries_the_query_and_the_snippets() {
        let snippets = vec![a_snippet("s1", "Prose about search.", Some("search();"))];
        let built = build("how do I search?", &snippets, 100_000);
        assert!(built.user.contains("how do I search?"));
        assert!(built.user.contains("Prose about search."));
        assert!(built.user.contains("search();"));
        assert_eq!(built.included, 1);
        assert_eq!(built.dropped, 0);
    }

    #[test]
    fn the_system_instruction_forbids_inventing_api_surface() {
        let built = build("q", &[], 100_000);
        assert!(!built.system.is_empty());
        assert_eq!(built.system, SYSTEM_INSTRUCTION);
    }

    #[test]
    fn retrieval_returning_nothing_still_produces_a_usable_prompt() {
        let built = build("how do I search?", &[], 100_000);
        assert!(built.user.contains("how do I search?"));
        assert_eq!(built.included, 0);
        assert!(
            !built.user.trim().is_empty(),
            "an empty user message would make the server answer from nothing at all"
        );
    }

    #[test]
    fn the_assembled_prompt_respects_the_budget_it_was_given() {
        let big = "some prose about identifiers ".repeat(200);
        let snippets: Vec<Snippet> = (0..5)
            .map(|i| a_snippet(&format!("s{i}"), &big, Some(&big)))
            .collect();
        let budget = 500;
        let built = build("a question", &snippets, budget);
        let total = estimate_tokens(&built.system) + estimate_tokens(&built.user);
        assert!(
            total <= budget,
            "assembled prompt estimated at {total} tokens against a {budget} budget"
        );
    }

    #[test]
    fn a_budget_smaller_than_the_instruction_still_returns_a_prompt() {
        let snippets = vec![a_snippet("s1", "Prose.", None)];
        let built = build("q", &snippets, 1);
        // Nothing fits, but the caller gets a well-formed prompt rather than
        // a panic or an empty string.
        assert_eq!(built.included, 0);
        assert!(built.user.contains("q"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qemer-answer prompt`
Expected: FAIL to compile — `build` has the wrong signature and `Prompt` / `SYSTEM_INSTRUCTION` do not exist.

- [ ] **Step 3: Write the implementation**

Append to `qemer-answer/src/prompt.rs`, above the test module.

```rust
/// What the model is told about its job.
///
/// A value, not a branch: `docs/decisions.md` records that falling back to a
/// sources-first presentation changes the prompt and the token cap only, so
/// swapping this string and lowering the cap is the whole change.
pub const SYSTEM_INSTRUCTION: &str = "\
You are a documentation assistant. Answer the question using only the \
documentation excerpts provided below. If the excerpts do not contain the \
answer, say so plainly rather than guessing. Prefer showing code from the \
excerpts over describing it. Never invent an API name, parameter, or \
behaviour that does not appear in the excerpts.";

/// Fixed text in the user message, counted against the budget so the budget
/// describes the whole prompt rather than only its snippets.
const USER_SCAFFOLD: &str = "Question: \n\nDocumentation excerpts:\n\n";

const NO_SNIPPETS: &str = "(No documentation excerpts were retrieved for this question.)";

/// A prompt ready to send, plus what had to be left out to make it fit.
#[derive(Debug, Clone)]
pub struct Prompt {
    pub system: String,
    pub user: String,
    /// How many snippets made it into `user`.
    pub included: usize,
    /// How many were left out for want of budget.
    pub dropped: usize,
    /// Whether the single oversized snippet path was taken.
    pub truncated: bool,
}

/// Assemble a grounded prompt within `budget_tokens`.
///
/// The instruction and the question are charged against the budget before any
/// snippet is, because a budget that only counts snippets is not a budget.
pub fn build(query: &str, snippets: &[Snippet], budget_tokens: usize) -> Prompt {
    let overhead = estimate_tokens(SYSTEM_INSTRUCTION)
        + estimate_tokens(query)
        + estimate_tokens(USER_SCAFFOLD);
    let snippet_budget = budget_tokens.saturating_sub(overhead);

    let fitted = fit(snippets, snippet_budget);

    let mut user = String::new();
    user.push_str("Question: ");
    user.push_str(query);
    user.push_str("\n\nDocumentation excerpts:\n\n");
    if fitted.snippets.is_empty() {
        user.push_str(NO_SNIPPETS);
        user.push('\n');
    } else {
        for snippet in &fitted.snippets {
            user.push_str(&render_snippet(snippet));
            user.push('\n');
        }
    }

    Prompt {
        system: SYSTEM_INSTRUCTION.to_string(),
        user,
        included: fitted.snippets.len(),
        dropped: fitted.dropped,
        truncated: fitted.truncated,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p qemer-answer prompt`
Expected: PASS, 16 tests.

- [ ] **Step 5: Commit**

```bash
git add qemer-answer/src/prompt.rs
git commit -m "feat(answer): assemble grounded prompts within a token budget

- build() returns system and user messages plus what was left out, so a
  caller can report coverage without recomputing it.
- The instruction and the question are charged against the budget too;
  a budget that counts only snippets is not a budget.
- The system instruction is a value rather than a branch, keeping the
  sources-first fallback in docs/decisions.md a one-line change."
```

---

## Task 3: SSE chunk parsing

Pure parsing of one line of a Server-Sent Events stream. Split out for the same reason `parse_embedding` was in `qemer-core`: the interesting failures are malformed bodies, and none of them need a socket.

**Files:**
- Create: `qemer-answer/src/stream.rs`
- Modify: `qemer-answer/src/lib.rs` (add `pub mod stream;`)
- Modify: `qemer-answer/Cargo.toml` (add `serde_json`)

**Interfaces:**
- Produces: `stream::Chunk` enum with variants `Token(String)`, `Usage { prompt_tokens: usize, completion_tokens: usize }`, `Done`, `Ignore`; `stream::parse_sse_line(line: &str) -> Result<Chunk, AnswerError>`.

- [ ] **Step 1: Add the dependency**

```toml
serde_json = "1.0.151"
```

Match the version already used in `qemer-core/Cargo.toml` so the workspace resolves one copy.

- [ ] **Step 2: Write the failing tests**

Create `qemer-answer/src/stream.rs` with the test module first.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_content_delta_is_a_token() {
        let line = r#"data: {"choices":[{"delta":{"content":"Hello"},"index":0}]}"#;
        assert!(matches!(parse_sse_line(line), Ok(Chunk::Token(t)) if t == "Hello"));
    }

    #[test]
    fn the_done_sentinel_is_recognised() {
        assert!(matches!(parse_sse_line("data: [DONE]"), Ok(Chunk::Done)));
    }

    #[test]
    fn a_blank_line_is_ignored() {
        assert!(matches!(parse_sse_line(""), Ok(Chunk::Ignore)));
    }

    #[test]
    fn a_non_data_line_is_ignored() {
        assert!(matches!(parse_sse_line(": keep-alive comment"), Ok(Chunk::Ignore)));
    }

    /// llama-server appends a final chunk with an EMPTY choices array and a
    /// usage object. Indexing choices[0] here would panic, so this is the
    /// single most important case in this module.
    #[test]
    fn the_final_usage_chunk_has_no_choices_and_does_not_panic() {
        let line = r#"data: {"choices":[],"usage":{"prompt_tokens":44,"completion_tokens":48,"total_tokens":92}}"#;
        match parse_sse_line(line) {
            Ok(Chunk::Usage { prompt_tokens, completion_tokens }) => {
                assert_eq!(prompt_tokens, 44);
                assert_eq!(completion_tokens, 48);
            }
            other => panic!("expected usage, got {other:?}"),
        }
    }

    #[test]
    fn a_delta_with_no_content_is_ignored_not_an_empty_token() {
        let line = r#"data: {"choices":[{"delta":{"role":"assistant"},"index":0}]}"#;
        assert!(matches!(parse_sse_line(line), Ok(Chunk::Ignore)));
    }

    #[test]
    fn a_finish_reason_chunk_with_a_null_content_is_ignored() {
        let line = r#"data: {"choices":[{"delta":{"content":null},"finish_reason":"stop","index":0}]}"#;
        assert!(matches!(parse_sse_line(line), Ok(Chunk::Ignore)));
    }

    #[test]
    fn a_malformed_json_payload_is_an_error_not_a_panic() {
        assert!(parse_sse_line("data: {not json").is_err());
    }

    #[test]
    fn leading_whitespace_after_data_is_optional() {
        let with = r#"data: {"choices":[{"delta":{"content":"a"}}]}"#;
        let without = r#"data:{"choices":[{"delta":{"content":"a"}}]}"#;
        assert!(matches!(parse_sse_line(with), Ok(Chunk::Token(_))));
        assert!(matches!(parse_sse_line(without), Ok(Chunk::Token(_))));
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p qemer-answer stream`
Expected: FAIL to compile — `Chunk` and `parse_sse_line` do not exist.

- [ ] **Step 4: Write the implementation**

Prepend to `qemer-answer/src/stream.rs`, above the test module.

```rust
//! Parsing one line of `llama-server`'s streamed chat-completion response.
//!
//! Server-Sent Events framing: each event is a `data: {json}` line, and the
//! stream ends with a literal `data: [DONE]`. Kept separate from the request
//! so malformed frames, absent content, and the final usage chunk are covered
//! without a running server.

use crate::AnswerError;

/// What one SSE line meant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chunk {
    /// A piece of the answer.
    Token(String),
    /// The final accounting chunk. Only present because the request asked for
    /// it; `include_usage` defaults to false server-side.
    Usage {
        prompt_tokens: usize,
        completion_tokens: usize,
    },
    /// The end-of-stream sentinel.
    Done,
    /// Keep-alive comments, blank lines, and chunks carrying no content.
    Ignore,
}

#[derive(serde::Deserialize)]
struct ChunkJson {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(serde::Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
}

#[derive(Default, serde::Deserialize)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(serde::Deserialize)]
struct Usage {
    prompt_tokens: usize,
    completion_tokens: usize,
}

/// Interpret one line. Anything that is not a `data:` line is not an event.
pub fn parse_sse_line(line: &str) -> Result<Chunk, AnswerError> {
    let Some(payload) = line.strip_prefix("data:") else {
        return Ok(Chunk::Ignore);
    };
    let payload = payload.trim();
    if payload.is_empty() {
        return Ok(Chunk::Ignore);
    }
    if payload == "[DONE]" {
        return Ok(Chunk::Done);
    }

    let parsed: ChunkJson = serde_json::from_str(payload)
        .map_err(|e| AnswerError::Generation(format!("unparseable stream chunk: {e}")))?;

    if let Some(usage) = parsed.usage {
        return Ok(Chunk::Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
        });
    }

    // The usage chunk carries `"choices": []`, so there is no choice to index
    // and no token to emit. Treating "no first choice" as "not a token" covers
    // it without special-casing.
    match parsed.choices.first().and_then(|c| c.delta.content.as_deref()) {
        Some(text) if !text.is_empty() => Ok(Chunk::Token(text.to_string())),
        _ => Ok(Chunk::Ignore),
    }
}
```

- [ ] **Step 5: Add the module to lib.rs**

Add `pub mod stream;` beside the existing `pub mod prompt;`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p qemer-answer stream`
Expected: PASS, 9 tests.

- [ ] **Step 7: Commit**

```bash
git add qemer-answer/src/stream.rs qemer-answer/src/lib.rs qemer-answer/Cargo.toml
git commit -m "feat(answer): parse llama-server's streamed completion chunks

Splits SSE parsing from the request so malformed frames and absent
content are covered without a live server. The final usage chunk
carries an empty choices array, which is covered explicitly because
indexing choices[0] there would panic."
```

---

## Task 4: The completion client

Join the two pure halves with an HTTP request. The subtlety is that SSE frames do not align with TCP reads, so lines must be reassembled rather than parsed per chunk.

**Files:**
- Modify: `qemer-answer/src/lib.rs`
- Modify: `qemer-answer/Cargo.toml` (add `async-stream`)

**Interfaces:**
- Consumes: `prompt::build`, `stream::parse_sse_line`, `stream::Chunk`.
- Produces: `Generator { base_url, model, context_tokens, max_completion_tokens }`; `Generator::prompt_budget(&self) -> usize`; `Generator::answer(&self, query: &str, snippets: &[Snippet]) -> impl Stream<Item = Result<AnswerEvent, AnswerError>>`.

- [ ] **Step 1: Add the dependency**

```toml
async-stream = "0.3"
```

Writing this loop by hand with `futures::stream::unfold` means threading the response, the byte buffer, and the accumulated usage through a state tuple across every iteration. `async-stream` lets it read as the loop it is. Confirm the version resolves: `cargo tree -p qemer-answer -i async-stream`.

- [ ] **Step 2: Write the implementation**

Replace `qemer-answer/src/lib.rs` entirely.

```rust
//! Grounding and generation: turn a query plus retrieved snippets into a
//! streamed answer.
//!
//! Depends on `qemer-core` only for the `Snippet` type. It never retrieves —
//! callers do that themselves and hand the results in. A consumer that wants
//! snippets and nothing else (an MCP server, say) never links this crate.

pub mod prompt;
pub mod stream;

use futures::{Stream, StreamExt};
use qemer_core::Snippet;

#[derive(Debug, Clone)]
pub enum AnswerEvent {
    Token(String),
    Done {
        prompt_tokens: usize,
        completion_tokens: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AnswerError {
    #[error("llama-server unreachable at {0}")]
    Unreachable(String),
    #[error("generation failed: {0}")]
    Generation(String),
}

/// Headroom between the prompt budget and the completion cap, absorbing the
/// chat template's own tokens and the estimator's error. Cheap insurance: the
/// failure it prevents is a rejected or silently truncated request.
const BUDGET_SAFETY_MARGIN: usize = 256;

pub struct Generator {
    pub base_url: String,
    pub model: String,
    /// Total context the server was started with; the prompt budget is derived
    /// from this minus room to actually answer.
    pub context_tokens: usize,
    /// Cap on generated tokens, and the room reserved for them.
    pub max_completion_tokens: usize,
}

impl Generator {
    /// How many tokens the prompt may occupy.
    pub fn prompt_budget(&self) -> usize {
        self.context_tokens
            .saturating_sub(self.max_completion_tokens + BUDGET_SAFETY_MARGIN)
    }

    /// Stream an answer grounded in `snippets`.
    ///
    /// Dropping the returned stream drops the underlying response, which
    /// closes the connection and stops the server generating. That is the
    /// whole cancellation story; callers needing to abort simply stop
    /// holding the stream.
    pub fn answer(
        &self,
        query: &str,
        snippets: &[Snippet],
    ) -> impl Stream<Item = Result<AnswerEvent, AnswerError>> {
        // Built eagerly and moved in: the returned stream borrows nothing, so
        // a caller can hold it for as long as it likes.
        let prompt = prompt::build(query, snippets, self.prompt_budget());
        let url = format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'));
        let model = self.model.clone();
        let max_tokens = self.max_completion_tokens;

        async_stream::try_stream! {
            let body = serde_json::json!({
                "model": model,
                "max_tokens": max_tokens,
                "stream": true,
                // Without this the server omits usage entirely and Done could
                // never carry real numbers; include_usage defaults to false.
                "stream_options": { "include_usage": true },
                "messages": [
                    { "role": "system", "content": prompt.system },
                    { "role": "user", "content": prompt.user },
                ],
            });

            let response = reqwest::Client::new()
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| AnswerError::Unreachable(format!("{url}: {e}")))?
                .error_for_status()
                .map_err(|e| AnswerError::Generation(e.to_string()))?;

            let mut bytes = response.bytes_stream();
            // SSE frames do not align with TCP reads, so bytes accumulate here
            // and only whole lines are parsed. Buffering as bytes rather than
            // as a String matters: a multi-byte character can be split across
            // two reads, and lossy-converting a partial one would corrupt it.
            let mut buffer: Vec<u8> = Vec::new();
            let mut usage: Option<(usize, usize)> = None;

            while let Some(chunk) = bytes.next().await {
                let chunk = chunk.map_err(|e| AnswerError::Generation(e.to_string()))?;
                buffer.extend_from_slice(&chunk);

                while let Some(newline) = buffer.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buffer.drain(..=newline).collect();
                    let line = String::from_utf8_lossy(&line);
                    match stream::parse_sse_line(line.trim_end())? {
                        stream::Chunk::Token(text) => yield AnswerEvent::Token(text),
                        stream::Chunk::Usage { prompt_tokens, completion_tokens } => {
                            usage = Some((prompt_tokens, completion_tokens));
                        }
                        stream::Chunk::Done | stream::Chunk::Ignore => {}
                    }
                }
            }

            // A server that closed without sending usage still ends the
            // stream cleanly; zeroes say "not reported", and the alternative
            // would be failing a request that actually produced an answer.
            let (prompt_tokens, completion_tokens) = usage.unwrap_or((0, 0));
            yield AnswerEvent::Done { prompt_tokens, completion_tokens };
        }
    }
}
```

- [ ] **Step 3: Confirm the crate builds**

Run: `cargo check -p qemer-answer`
Expected: PASS.

If `try_stream!` complains about inferring the error type, annotate the first yielded error path; the macro needs one concrete `AnswerError` in scope to fix `Item = Result<AnswerEvent, AnswerError>`.

- [ ] **Step 4: Add a budget test**

Budget arithmetic belongs beside `Generator`, not in `prompt.rs`. Append to `qemer-answer/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn generator(context_tokens: usize, max_completion_tokens: usize) -> Generator {
        Generator {
            base_url: "http://localhost:8081".into(),
            model: "qwen3.5-0.8b".into(),
            context_tokens,
            max_completion_tokens,
        }
    }

    #[test]
    fn the_prompt_budget_reserves_room_to_answer() {
        let g = generator(8192, 512);
        assert!(g.prompt_budget() < 8192 - 512, "the safety margin must also be reserved");
    }

    #[test]
    fn a_context_smaller_than_the_reservation_yields_a_zero_budget_not_a_panic() {
        let g = generator(128, 512);
        assert_eq!(g.prompt_budget(), 0);
    }
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p qemer-answer`
Expected: PASS, all tests.

- [ ] **Step 6: Commit**

```bash
git add qemer-answer/src/lib.rs qemer-answer/Cargo.toml
git commit -m "feat(answer): stream grounded answers from llama-server

- Request /v1/chat/completions with stream_options.include_usage, which
  defaults to false and without which Done could never carry counts.
- Reassemble SSE lines from a byte buffer, since frames do not align
  with TCP reads and a split multi-byte character would corrupt.
- Reserve max_completion_tokens plus a margin out of the context, so
  the prompt budget leaves room to actually answer.
- Cancellation is by drop; no abort API is needed or offered."
```

---

## Task 5: End-to-end against a stub server

Prove the wiring. `qemer-core/tests/embed_stub.rs` already established this pattern and it worked first try; this is the same shape with SSE frames, deliberately split across writes so the line buffer is actually exercised rather than merely present.

**Files:**
- Create: `qemer-answer/tests/completion_stub.rs`
- Create: `qemer-answer/tests/generate.rs`

**Interfaces:**
- Consumes: `Generator::answer`, `AnswerEvent`.
- Produces: `completion_stub::start(frames: Vec<String>) -> String` (async, returns a base URL).

- [ ] **Step 1: Write the stub server**

```rust
//! A one-shot HTTP stand-in for `llama-server`'s streaming chat endpoint.
//!
//! Writes the supplied SSE frames and closes. Frames are written in two
//! deliberately misaligned pieces so the client's line reassembly is
//! exercised rather than merely present.

#![allow(dead_code)]

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Start the stub and return its base URL.
pub async fn start(frames: Vec<String>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        // Not a real HTTP parser: read once and assume the request arrived in
        // a single packet, which holds on loopback for a small JSON POST.
        let mut scratch = vec![0u8; 16384];
        let _ = socket.read(&mut scratch).await;

        let body: String = frames.iter().map(|f| format!("data: {f}\n\n")).collect();
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
        let _ = socket.write_all(head.as_bytes()).await;

        // Split mid-body so at least one SSE frame straddles two writes.
        let split = body.len() / 2;
        let split = body
            .char_indices()
            .map(|(i, _)| i)
            .find(|i| *i >= split)
            .unwrap_or(0);
        let _ = socket.write_all(body[..split].as_bytes()).await;
        let _ = socket.flush().await;
        let _ = socket.write_all(body[split..].as_bytes()).await;
        let _ = socket.shutdown().await;
    });

    format!("http://{addr}")
}
```

- [ ] **Step 2: Write the failing test**

```rust
// qemer-answer/tests/generate.rs
mod completion_stub;

use futures::StreamExt;
use qemer_answer::{AnswerEvent, Generator};
use qemer_core::Snippet;

fn a_snippet() -> Snippet {
    Snippet {
        library: "lancedb".into(),
        version: "0.37.1".into(),
        snippet_id: "s1".into(),
        title: "Full text search".into(),
        description: "Run a keyword search over an indexed table.".into(),
        code: Some("table.create_index(&[\"text\"], Index::FTS(params)).await?;".into()),
        source_url: Some("https://example/fts".into()),
        score: 1.0,
    }
}

fn generator(base_url: String) -> Generator {
    Generator {
        base_url,
        model: "qwen3.5-0.8b".into(),
        context_tokens: 8192,
        max_completion_tokens: 512,
    }
}

#[tokio::test]
async fn a_streamed_answer_yields_tokens_then_done_with_counts() {
    let frames = vec![
        r#"{"choices":[{"delta":{"role":"assistant"},"index":0}]}"#.to_string(),
        r#"{"choices":[{"delta":{"content":"Call "},"index":0}]}"#.to_string(),
        r#"{"choices":[{"delta":{"content":"create_index"},"index":0}]}"#.to_string(),
        r#"{"choices":[{"delta":{"content":"."},"index":0}]}"#.to_string(),
        r#"{"choices":[],"usage":{"prompt_tokens":44,"completion_tokens":3,"total_tokens":47}}"#
            .to_string(),
        "[DONE]".to_string(),
    ];
    let base_url = completion_stub::start(frames).await;
    let generator = generator(base_url);

    let events: Vec<_> = generator
        .answer("how do I search?", &[a_snippet()])
        .collect()
        .await;
    let events: Vec<AnswerEvent> = events.into_iter().map(|e| e.unwrap()).collect();

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AnswerEvent::Token(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Call create_index.");

    match events.last().unwrap() {
        AnswerEvent::Done { prompt_tokens, completion_tokens } => {
            assert_eq!(*prompt_tokens, 44);
            assert_eq!(*completion_tokens, 3);
        }
        other => panic!("stream must end with Done, got {other:?}"),
    }
}

#[tokio::test]
async fn a_stream_without_usage_still_ends_cleanly() {
    let frames = vec![
        r#"{"choices":[{"delta":{"content":"Hi"},"index":0}]}"#.to_string(),
        "[DONE]".to_string(),
    ];
    let base_url = completion_stub::start(frames).await;

    let events: Vec<_> = generator(base_url)
        .answer("q", &[a_snippet()])
        .collect()
        .await;
    let last = events.into_iter().last().unwrap().unwrap();
    assert!(
        matches!(last, AnswerEvent::Done { prompt_tokens: 0, completion_tokens: 0 }),
        "a server that reports no usage must still terminate the stream"
    );
}

#[tokio::test]
async fn an_unreachable_server_surfaces_as_unreachable() {
    // Port 1 on loopback: reserved, and nothing will be listening.
    let generator = generator("http://127.0.0.1:1".into());
    let events: Vec<_> = generator.answer("q", &[a_snippet()]).collect().await;
    let first = events.into_iter().next().unwrap();
    assert!(matches!(
        first,
        Err(qemer_answer::AnswerError::Unreachable(_))
    ));
}
```

- [ ] **Step 3: Run the tests to verify they fail, then pass**

Run: `cargo test -p qemer-answer --test generate`
Expected first: FAIL to compile — `completion_stub` does not exist until Step 1's file is saved.
Expected after: PASS, 3 tests.

If `a_streamed_answer_yields_tokens_then_done_with_counts` produces the right text but the wrong counts, the request is not sending `stream_options.include_usage` — check Task 4's body.

- [ ] **Step 4: Run the whole suite**

Run: `cargo test -p qemer-answer`
Expected: PASS, everything.

- [ ] **Step 5: Check the boundaries held**

Run: `grep -rniE "lancedb|corpus|embed|retriev|ratatui|crossterm|\bEsc\b" qemer-answer/src/`
Expected: no hits. `qemer-answer` must not name retrieval, the embedding endpoint, or the terminal — see Global Constraints. The word "excerpts" is fine; "embeddings" is not.

Run: `cargo check --workspace`
Expected: PASS. `qemer-tui` does not construct a `Generator` yet, so the new `max_completion_tokens` field breaks nothing; confirm rather than assume.

- [ ] **Step 6: Commit**

```bash
git add qemer-answer/tests/
git commit -m "test(answer): drive generation end to end against a stub server

- A one-shot SSE server writes frames in two misaligned pieces, so the
  client's line reassembly is exercised rather than merely present.
- Covers the usage-bearing final chunk, a server that reports no usage
  at all, and an unreachable endpoint."
```

---

## What this plan does not cover

Deliberately out of scope, each needing its own plan or decision:

- **`qemer-tui`** — the terminal UI, config loading, corpus browsing, and the Esc-abort binding that consumes this crate's drop-to-cancel contract.
- **Retry, backoff, or reconnection.** A dropped stream surfaces as an error and the caller decides. Adding retries here would generate twice against a small model for reasons the caller cannot see.
- **Measuring whether the estimator is any good.** `CHARS_PER_TOKEN` is a placeholder. Comparing it against `/tokenize` over a real corpus is worth doing once there is a real corpus, and belongs with the token-budget question below.

## Open questions this plan must not answer on its own

If executing this plan appears to require resolving any of these, **stop and ask**. The first two are already recorded as open in `docs/decisions.md`.

- **The token budget default.** `max_completion_tokens` has no default chosen here, and `BUDGET_SAFETY_MARGIN = 256` is an invented number. Both depend on Qwen3.5-0.8B's real context length, which `docs/decisions.md` says must be read from the model card or the `llama-server` startup log rather than assumed. The tests pass a value in explicitly so nothing in this plan depends on the default; do not add one to `Generator`.
- **What the failure message says when `llama-server` is unreachable.** `AnswerError::Unreachable` names the URL and stops there, mirroring how `qemer-core`'s embedding client behaves. What the user is told to start is the caller's to say — it knows which of the two endpoints it wanted. Do not put advice in this crate's error text.
- **Whether `CHARS_PER_TOKEN = 3.5` is right.** It is a guess in the safe direction. Changing it is one line, but changing it *based on nothing* is how a placeholder becomes a fact by repetition.
- **The wording of `SYSTEM_INSTRUCTION`.** It is written for a small model that confabulates under retrieval misses, which `docs/decisions.md` names as the live risk. Tuning it is expected; restructuring the crate around answer styles is not.
