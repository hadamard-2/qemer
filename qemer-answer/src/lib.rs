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
