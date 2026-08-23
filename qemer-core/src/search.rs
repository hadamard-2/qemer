//! Vector search over an installed corpus.

use crate::Result;

/// One documentation snippet, mirroring the block structure of Context7's
/// curated markdown: a heading, a source URL, prose, and usually one code
/// block.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Snippet {
    pub library: String,
    pub version: String,
    pub title: String,
    pub description: String,
    /// Absent for description-only blocks, which Context7 does emit.
    pub code: Option<String>,
    pub language: Option<String>,
    pub source_url: Option<String>,
    /// Cosine distance from the query; lower is closer.
    pub distance: f32,
}

/// Search a single library's corpus. Library scoping is deliberate: routing a
/// query across libraries is a separate problem we are not solving yet.
pub async fn search(_library: &str, _query: &str, _k: usize) -> Result<Vec<Snippet>> {
    todo!("embed query, then filtered ANN search against the library's table")
}
