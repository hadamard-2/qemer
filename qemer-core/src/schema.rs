//! The Arrow schema of a corpus table.
//!
//! Column names live here rather than as string literals at call sites so a
//! rename is one edit and a typo is a compile error.

// Arrow comes from lancedb rather than a direct dependency: two arrow
// versions in one graph produce same-named, incompatible types, and taking
// the re-export means there can only ever be one.
use lancedb::arrow::arrow_schema::{DataType, Field, Schema, SchemaRef};
use std::sync::Arc;

pub const COL_SNIPPET_ID: &str = "snippet_id";
pub const COL_KIND: &str = "kind";
pub const COL_TITLE: &str = "title";
pub const COL_SOURCE_URL: &str = "source_url";
pub const COL_TEXT: &str = "text";
pub const COL_VECTOR: &str = "vector";

/// Dimension of nomic-embed-text-v1.5 as published corpora use it.
pub const EMBEDDING_DIM: i32 = 768;

/// One row per prose-or-code unit. A block with no code contributes one row.
pub fn corpus_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new(COL_SNIPPET_ID, DataType::Utf8, false),
        Field::new(COL_KIND, DataType::Utf8, false),
        Field::new(COL_TITLE, DataType::Utf8, false),
        Field::new(COL_SOURCE_URL, DataType::Utf8, true),
        Field::new(COL_TEXT, DataType::Utf8, false),
        Field::new(
            COL_VECTOR,
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                EMBEDDING_DIM,
            ),
            false,
        ),
    ]))
}

/// Parameters for every FTS index built over a corpus, and for every query run
/// against one. Index-time and query-time tokenization must agree, so both
/// sides read this rather than constructing their own.
///
/// The `code` base tokenizer keeps identifiers whole: `create_index` indexes
/// as one term, where the default `simple` tokenizer would split and stem it
/// into `creat` + `index`, and would drop the `to` in `nearest_to` as a stop
/// word. The cost is that prose loses stemming, so `creates` no longer matches
/// `create` on the BM25 side. That trade is deliberate — in a hybrid, matching
/// across word forms is the vector retriever's job, and exact identifier
/// matching is the one thing only BM25 can do.
pub fn fts_index_params() -> lancedb::index::scalar::FtsIndexBuilder {
    lancedb::index::scalar::FtsIndexBuilder::default().base_tokenizer("code".to_string())
}

/// The columns carrying searchable text. Each gets its own FTS index —
/// lancedb 0.37.1 rejects composite indices — and a query names them all.
pub const FTS_COLUMNS: &[&str] = &[COL_TEXT, COL_TITLE];
