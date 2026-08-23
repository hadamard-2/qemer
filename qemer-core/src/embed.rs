//! Query embedding via a running `llama-server`.
//!
//! Qemer never installs or launches a model runtime. It talks HTTP to a server
//! the user is already running, and reports clearly when one is not reachable.

use crate::{CoreError, Result};

pub struct EmbedClient {
    pub base_url: String,
    /// Stamped into every corpus and checked before search; see
    /// `CoreError::ModelMismatch`.
    pub model: String,
    pub dim: usize,
}

#[derive(serde::Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingDatum>,
}

#[derive(serde::Deserialize)]
struct EmbeddingDatum {
    embedding: Vec<f32>,
}

/// Parse an OpenAI-shaped embeddings response and check its width.
pub fn parse_embedding(body: &[u8], expected_dim: usize) -> Result<Vec<f32>> {
    let parsed: EmbeddingResponse = serde_json::from_slice(body)
        .map_err(|e| CoreError::Embed(format!("unexpected response body: {e}")))?;
    let first = parsed
        .data
        .into_iter()
        .next()
        .ok_or_else(|| CoreError::Embed("response contained no embeddings".into()))?;
    if first.embedding.len() != expected_dim {
        return Err(CoreError::Embed(format!(
            "expected {expected_dim} dimensions, received {}",
            first.embedding.len()
        )));
    }
    Ok(first.embedding)
}

impl EmbedClient {
    /// Refuse to search a corpus built with different embeddings. A mismatch
    /// yields plausible-looking nonsense rather than an obvious failure, so
    /// this runs before every search rather than at install time only.
    pub fn check_corpus(&self, reference: &crate::corpus::CorpusRef) -> Result<()> {
        if reference.embedding_model == self.model && reference.embedding_dim == self.dim {
            return Ok(());
        }
        Err(crate::CoreError::ModelMismatch {
            corpus: format!("{}-{}", reference.library, reference.version),
            corpus_model: reference.embedding_model.clone(),
            corpus_dim: reference.embedding_dim,
            client_model: self.model.clone(),
            client_dim: self.dim,
        })
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/v1/embeddings", self.base_url.trim_end_matches('/'));
        let response = reqwest::Client::new()
            .post(&url)
            .json(&serde_json::json!({ "input": text, "model": self.model }))
            .send()
            .await
            .map_err(|e| {
                // Names the URL and what was attempted. What the user should
                // start is the caller's to say, not this crate's.
                CoreError::Embed(format!("no embedding server reachable at {url}: {e}"))
            })?;
        let body = response
            .bytes()
            .await
            .map_err(|e| CoreError::Embed(e.to_string()))?;
        parse_embedding(&body, self.dim)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::CorpusRef;

    fn client() -> EmbedClient {
        EmbedClient {
            base_url: "http://localhost:8080".into(),
            model: "nomic-embed-text-v1.5".into(),
            dim: 768,
        }
    }

    fn corpus(model: &str, dim: usize) -> CorpusRef {
        CorpusRef {
            library: "lancedb".into(),
            version: "0.37.1".into(),
            url: String::new(),
            sha256: String::new(),
            bytes: 0,
            embedding_model: model.into(),
            embedding_dim: dim,
            snippet_count: 0,
        }
    }

    #[test]
    fn a_matching_corpus_passes() {
        assert!(client().check_corpus(&corpus("nomic-embed-text-v1.5", 768)).is_ok());
    }

    #[test]
    fn a_different_model_fails() {
        let err = client().check_corpus(&corpus("all-MiniLM-L6-v2", 768)).unwrap_err();
        assert!(matches!(err, crate::CoreError::ModelMismatch { .. }));
    }

    #[test]
    fn a_different_dimension_fails_even_with_the_same_model_name() {
        let err = client().check_corpus(&corpus("nomic-embed-text-v1.5", 512)).unwrap_err();
        assert!(matches!(err, crate::CoreError::ModelMismatch { .. }));
    }

    #[test]
    fn the_error_names_both_sides() {
        let err = client().check_corpus(&corpus("other", 512)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("other"), "error must name the corpus model");
        assert!(
            text.contains("nomic-embed-text-v1.5"),
            "error must name the client model"
        );
    }

    #[test]
    fn parses_an_openai_shaped_embedding_response() {
        let body = br#"{"data":[{"embedding":[1.0,2.0,3.0],"index":0}]}"#;
        assert_eq!(parse_embedding(body, 3).unwrap(), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn a_wrong_dimension_is_an_error() {
        let body = br#"{"data":[{"embedding":[1.0,2.0],"index":0}]}"#;
        assert!(parse_embedding(body, 768).is_err());
    }

    #[test]
    fn an_empty_data_array_is_an_error_not_a_panic() {
        assert!(parse_embedding(br#"{"data":[]}"#, 768).is_err());
    }

    #[test]
    fn a_non_embedding_body_is_an_error() {
        assert!(parse_embedding(br#"{"error":"model not loaded"}"#, 768).is_err());
    }
}
