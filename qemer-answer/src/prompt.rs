//! Prompt assembly and context budgeting.
//!
//! Kept as pure functions over data — snippets in, a prompt out — because this
//! is where the edge cases live: a single code block larger than the whole
//! budget, retrieval returning nothing.
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

/// The result of fitting snippets to a budget.
#[derive(Debug, Clone)]
pub struct Fitted {
    /// Snippets that fit, in the order they were given.
    pub snippets: Vec<Snippet>,
    /// How many were left out. Callers may want to say so.
    pub dropped: usize,
}

/// Take snippets in rank order until the budget is spent, then stop.
///
/// Stopping rather than skipping ahead is deliberate: the input order is the
/// fused ranking, and reaching past a snippet to fit a lower-ranked one would
/// undo the ordering retrieval just established.
///
/// Snippets are kept or dropped whole, with no exception for the top-ranked
/// one. A snippet larger than the entire budget is dropped like any other,
/// which can leave nothing at all — the caller is told so via `dropped` and
/// says whatever it wants to say about answering without grounding.
pub fn fit(snippets: &[Snippet], budget_tokens: usize) -> Fitted {
    let mut kept: Vec<Snippet> = Vec::new();
    let mut used = 0usize;

    for snippet in snippets {
        let cost = estimate_tokens(&render_snippet(snippet));
        if used + cost > budget_tokens {
            break;
        }
        used += cost;
        kept.push(snippet.clone());
    }

    Fitted {
        dropped: snippets.len() - kept.len(),
        snippets: kept,
    }
}

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
    }
}

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
    }

    /// Snippets are dropped whole, with no exception for the top-ranked one.
    /// A snippet too large for the budget cannot be made to fit without
    /// cutting it, and cutting is not something this module does.
    #[test]
    fn a_single_snippet_larger_than_the_whole_budget_is_dropped() {
        let huge = "let identifier_name = 1;\n".repeat(500);
        let snippets = vec![a_snippet("s1", "Prose.", Some(&huge))];
        let fitted = fit(&snippets, 100);

        assert!(fitted.snippets.is_empty(), "no partial snippet may be kept");
        assert_eq!(fitted.dropped, 1);
    }

    #[test]
    fn every_kept_snippet_fits_the_budget_it_was_given() {
        let big = "some prose about identifiers ".repeat(50);
        let snippets: Vec<Snippet> = (0..6)
            .map(|i| a_snippet(&format!("s{i}"), &big, Some(&big)))
            .collect();
        let budget = 900;
        let fitted = fit(&snippets, budget);
        let total: usize = fitted
            .snippets
            .iter()
            .map(|s| estimate_tokens(&render_snippet(s)))
            .sum();
        assert!(total <= budget, "kept {total} tokens against a {budget} budget");
    }

    #[test]
    fn a_zero_budget_keeps_nothing() {
        let snippets = vec![a_snippet("s1", "Prose.", None)];
        let fitted = fit(&snippets, 0);
        assert!(fitted.snippets.is_empty());
    }

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

    /// A snippet too big to fit is dropped whole, so the prompt falls back to
    /// the same no-excerpts form as a retrieval miss rather than carrying a
    /// fragment.
    #[test]
    fn an_oversized_snippet_leaves_the_prompt_grounded_in_nothing() {
        let huge = "let identifier_name = 1;\n".repeat(500);
        let snippets = vec![a_snippet("s1", "Prose.", Some(&huge))];
        let built = build("q", &snippets, 200);
        assert_eq!(built.included, 0);
        assert_eq!(built.dropped, 1);
        assert!(built.user.contains(NO_SNIPPETS));
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

}
