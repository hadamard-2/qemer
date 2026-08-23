//! Vector search over an installed corpus.

use crate::Result;

/// One documentation snippet, reassembled from its prose and code rows.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Snippet {
    pub library: String,
    pub version: String,
    pub snippet_id: String,
    pub title: String,
    pub description: String,
    /// Absent for description-only blocks, which the corpus does contain.
    pub code: Option<String>,
    pub source_url: Option<String>,
    /// Reciprocal-rank-fusion score. Higher is better. Not a distance, and
    /// not comparable across queries.
    pub score: f32,
}

/// Search a single library's corpus. Library scoping is deliberate: routing a
/// query across libraries is a separate problem we are not solving yet.
pub async fn search(_library: &str, _query: &str, _k: usize) -> Result<Vec<Snippet>> {
    todo!("embed query, then filtered ANN search against the library's table")
}
