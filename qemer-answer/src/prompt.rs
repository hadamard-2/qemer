//! Prompt assembly and context budgeting.
//!
//! Kept as pure functions over data — snippets in, a string out — because this
//! is where the edge cases live: a single code block larger than the whole
//! budget, retrieval returning nothing, truncation landing mid-identifier.

use qemer_core::Snippet;

/// Fit as many snippets as the budget allows, truncating the last if needed.
pub fn build(_query: &str, _snippets: &[Snippet], _budget_tokens: usize) -> String {
    todo!("render snippets into a grounded prompt within budget")
}
