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
    pub async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
        todo!("POST to {}/v1/embeddings", self.base_url)
    }
}
