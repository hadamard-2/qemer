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
