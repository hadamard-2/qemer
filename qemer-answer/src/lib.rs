//! Grounding and generation: turn a query plus retrieved snippets into a
//! streamed answer.
//!
//! Depends on `qemer-core` only for the `Snippet` type. It never retrieves —
//! callers do that themselves and hand the results in. A consumer that wants
//! snippets and nothing else (an MCP server, say) never links this crate.

pub mod prompt;

use futures::Stream;
use qemer_core::Snippet;

#[derive(Debug, Clone)]
pub enum AnswerEvent {
    Token(String),
    Done { prompt_tokens: usize, completion_tokens: usize },
}

#[derive(Debug, thiserror::Error)]
pub enum AnswerError {
    #[error("llama-server unreachable at {0}")]
    Unreachable(String),
    #[error("generation failed: {0}")]
    Generation(String),
}

pub struct Generator {
    pub base_url: String,
    pub model: String,
    /// Total context the server was started with; the prompt budget is derived
    /// from this minus room to actually answer.
    pub context_tokens: usize,
}

impl Generator {
    pub fn answer(
        &self,
        _query: &str,
        _snippets: &[Snippet],
    ) -> impl Stream<Item = Result<AnswerEvent, AnswerError>> {
        futures::stream::empty()
    }
}
