//! Query embedding via a running `llama-server`.
//!
//! Qemer never installs or launches a model runtime. It talks HTTP to a server
//! the user is already running, and reports clearly when one is not reachable.

use crate::Result;

pub struct EmbedClient {
    pub base_url: String,
    /// Stamped into every corpus and checked before search; see
    /// `CoreError::ModelMismatch`.
    pub model: String,
    pub dim: usize,
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

    pub async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        todo!("POST to {}/v1/embeddings", self.base_url)
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
}
