//! One query, start to finish, as a single stream.
//!
//! Generation is a stream but search is a single await — an embedding
//! round-trip plus a database query. Awaiting search directly would freeze
//! the interface for its duration, including through a network timeout when
//! the embedding server is down, which is exactly when a user wants out.
//! Wrapping both phases in one stream gives search the same escape hatch:
//! dropping the stream cancels whichever phase is live.
//!
//! This module is also the one place that knows both endpoints exist. The
//! library crates each name only their own, and report failures without
//! advice, precisely so that the caller — which knows which endpoint it
//! wanted — supplies it.

use futures::{Stream, StreamExt};
use qemer_answer::{AnswerError, AnswerEvent, Generator};
use qemer_core::embed::EmbedClient;
use qemer_core::{Corpus, CoreError, Snippet, search};

/// What the interface learns as a query progresses.
#[derive(Debug, Clone)]
pub enum QueryEvent {
    /// Retrieval has started. Emitted first so the interface can say so
    /// before the embedding round-trip returns.
    Searching,
    /// Retrieval finished. Emitted before any token, so the grounding is on
    /// screen while the model is still working — which matters most when the
    /// answer turns out to be wrong.
    Snippets(Vec<Snippet>),
    Token(String),
    Done {
        prompt_tokens: usize,
        completion_tokens: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("{0}")]
    Retrieval(String),
    #[error("{0}")]
    Generation(String),
}

/// Describe a retrieval failure. Names the embedding endpoint and never the
/// completion one.
pub fn describe_retrieval_failure(error: &CoreError, embedding_url: &str) -> String {
    match error {
        // Only unreachability is worth advice. A mismatch or a missing
        // corpus is already precise, and restarting a server fixes neither.
        CoreError::Embed(reason) => format!(
            "Could not reach the embedding server at {embedding_url} ({reason}). \
             Start llama-server with your embedding model on that address, then ask again."
        ),
        other => other.to_string(),
    }
}

/// Describe a generation failure. Names the completion endpoint and never the
/// embedding one.
pub fn describe_generation_failure(error: &AnswerError, completion_url: &str) -> String {
    match error {
        AnswerError::Unreachable(_) => format!(
            "Could not reach the completion server at {completion_url}. \
             Start llama-server with your chat model on that address, then ask again."
        ),
        other => other.to_string(),
    }
}

/// Run one query: retrieve, then generate, as a single cancellable stream.
///
/// Everything is taken by value so the returned stream borrows nothing and
/// the caller may hold it for as long as it likes.
pub fn run(
    corpus: Corpus,
    embed: EmbedClient,
    generator: Generator,
    query: String,
    k: usize,
) -> impl Stream<Item = Result<QueryEvent, QueryError>> {
    async_stream::try_stream! {
        yield QueryEvent::Searching;

        let snippets = search::search(&corpus, &embed, &query, k)
            .await
            .map_err(|e| QueryError::Retrieval(describe_retrieval_failure(&e, &embed.base_url)))?;
        yield QueryEvent::Snippets(snippets.clone());

        let answer = generator.answer(&query, &snippets);
        let mut answer = std::pin::pin!(answer);
        while let Some(event) = answer.next().await {
            let event = event.map_err(|e| {
                QueryError::Generation(describe_generation_failure(&e, &generator.base_url))
            })?;
            match event {
                AnswerEvent::Token(text) => yield QueryEvent::Token(text),
                AnswerEvent::Done { prompt_tokens, completion_tokens } => {
                    yield QueryEvent::Done { prompt_tokens, completion_tokens };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMBEDDING_URL: &str = "http://localhost:8080";
    const COMPLETION_URL: &str = "http://localhost:8081";

    #[test]
    fn an_unreachable_embedding_server_names_that_endpoint_and_what_to_start() {
        let error = CoreError::Embed("connection refused".into());
        let message = describe_retrieval_failure(&error, EMBEDDING_URL);
        assert!(message.contains(EMBEDDING_URL), "{message}");
        assert!(message.contains("llama-server"), "{message}");
    }

    /// The two endpoints fail independently and are configured separately.
    /// A retrieval failure that mentions the completion endpoint would send
    /// the user to restart the wrong server.
    #[test]
    fn a_retrieval_failure_never_mentions_the_completion_endpoint() {
        let error = CoreError::Embed("connection refused".into());
        let message = describe_retrieval_failure(&error, EMBEDDING_URL);
        assert!(!message.contains(COMPLETION_URL), "{message}");
        assert!(!message.to_lowercase().contains("completion"), "{message}");
    }

    #[test]
    fn a_generation_failure_never_mentions_the_embedding_endpoint() {
        let error = AnswerError::Unreachable(format!("{COMPLETION_URL}: connection refused"));
        let message = describe_generation_failure(&error, COMPLETION_URL);
        assert!(message.contains(COMPLETION_URL), "{message}");
        assert!(!message.contains(EMBEDDING_URL), "{message}");
        assert!(!message.to_lowercase().contains("embedding"), "{message}");
    }

    /// A model mismatch is already precise about what is wrong, and no
    /// server needs restarting. Telling the user to start llama-server would
    /// be actively misleading.
    #[test]
    fn a_model_mismatch_is_passed_through_without_start_advice() {
        let error = CoreError::ModelMismatch {
            corpus: "lancedb-0.37.1".into(),
            corpus_model: "nomic-embed-text-v1.5".into(),
            corpus_dim: 768,
            client_model: "other-model".into(),
            client_dim: 384,
        };
        let message = describe_retrieval_failure(&error, EMBEDDING_URL);
        assert!(message.contains("lancedb-0.37.1"), "{message}");
        assert!(message.contains("other-model"), "{message}");
        assert!(
            !message.contains("llama-server"),
            "restarting a server does not fix a mismatched corpus: {message}"
        );
    }

    #[test]
    fn a_missing_corpus_is_passed_through_without_start_advice() {
        let error = CoreError::CorpusMissing("lancedb".into());
        let message = describe_retrieval_failure(&error, EMBEDDING_URL);
        assert!(message.contains("lancedb"), "{message}");
        assert!(!message.contains("llama-server"), "{message}");
    }

    #[test]
    fn a_generation_error_that_is_not_unreachability_is_passed_through() {
        let error = AnswerError::Generation("HTTP 500".into());
        let message = describe_generation_failure(&error, COMPLETION_URL);
        assert!(message.contains("HTTP 500"), "{message}");
        assert!(!message.contains("llama-server"), "{message}");
    }
}
