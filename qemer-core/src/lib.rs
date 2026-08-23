//! Retrieval layer: corpus management and vector search over Context7-derived
//! documentation snippets.
//!
//! This crate knows nothing about answer generation. Its only consumers'
//! requirement is a query in and snippets out.

pub mod cache;
pub mod corpus;
pub mod embed;
pub mod fuse;
pub mod schema;
pub mod search;

pub use cache::Cache;
pub use corpus::{Corpus, CorpusRef, Manifest};
pub use search::Snippet;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("no corpus installed for library `{0}`")]
    CorpusMissing(String),
    #[error(
        "corpus `{corpus}` was embedded with `{corpus_model}` ({corpus_dim}d) \
         but this client is configured for `{client_model}` ({client_dim}d)"
    )]
    ModelMismatch {
        corpus: String,
        corpus_model: String,
        corpus_dim: usize,
        client_model: String,
        client_dim: usize,
    },
    #[error("manifest at {url} could not be read: {reason}")]
    Manifest { url: String, reason: String },
    #[error("corpus download from {url} failed: {reason}")]
    Download { url: String, reason: String },
    #[error("corpus download failed checksum: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("embedding request failed: {0}")]
    Embed(String),
    #[error(transparent)]
    Db(#[from] lancedb::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, CoreError>;
