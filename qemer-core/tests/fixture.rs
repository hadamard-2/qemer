//! Builds a small corpus table in a temp directory. Shared by retrieval tests.

use lancedb::arrow::arrow_array::{
    FixedSizeListArray, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray,
    types::Float32Type,
};
use qemer_core::schema::*;
use std::path::Path;
use std::sync::Arc;

/// (snippet_id, kind, title, source_url, text)
pub const ROWS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "s1",
        "prose",
        "Full text search",
        "https://example/fts",
        "Run a keyword search over a table that has a full text search index.",
    ),
    (
        "s1",
        "code",
        "Full text search",
        "https://example/fts",
        "table.create_index(&[\"text\"], Index::FTS(Default::default())).await?;",
    ),
    (
        "s2",
        "prose",
        "Vector search",
        "https://example/ann",
        "Find the rows whose embeddings are closest to a query vector.",
    ),
    (
        "s2",
        "code",
        "Vector search",
        "https://example/ann",
        "table.query().nearest_to(&[1.0, 2.0, 3.0])?.execute().await?;",
    ),
    (
        "s3",
        "prose",
        "Connecting to a database",
        "https://example/connect",
        "Open a database directory. Tables are created inside it.",
    ),
];

pub fn batch(vectors: Vec<Vec<f32>>) -> RecordBatch {
    let schema = corpus_schema();
    RecordBatch::try_new(
        schema,
        vec![
            Arc::new(StringArray::from_iter_values(ROWS.iter().map(|r| r.0))),
            Arc::new(StringArray::from_iter_values(ROWS.iter().map(|r| r.1))),
            Arc::new(StringArray::from_iter_values(ROWS.iter().map(|r| r.2))),
            Arc::new(StringArray::from_iter_values(ROWS.iter().map(|r| r.3))),
            Arc::new(StringArray::from_iter_values(ROWS.iter().map(|r| r.4))),
            Arc::new(
                FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
                    vectors
                        .into_iter()
                        .map(|v| Some(v.into_iter().map(Some).collect::<Vec<_>>())),
                    EMBEDDING_DIM,
                ),
            ),
        ],
    )
    .unwrap()
}

/// Deterministic filler vectors: row i is all-`i as f32 / 100.0`.
pub fn filler_vectors() -> Vec<Vec<f32>> {
    (0..ROWS.len())
        .map(|i| vec![i as f32 / 100.0; EMBEDDING_DIM as usize])
        .collect()
}

pub async fn build_fixture_table(dir: &Path) -> lancedb::Table {
    let db = lancedb::connect(dir.to_str().unwrap())
        .execute()
        .await
        .unwrap();
    let schema = corpus_schema();
    let reader = RecordBatchIterator::new(
        vec![Ok(batch(filler_vectors()))].into_iter(),
        schema.clone(),
    );
    // `Scannable` covers `Box<dyn RecordBatchReader + Send>`, not a bare
    // iterator.
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(reader);
    let table = db.create_table("snippets", reader).execute().await.unwrap();
    // One index per column: lancedb 0.37.1 rejects composite indices. Both are
    // searched together by naming them on the query.
    for column in FTS_COLUMNS {
        table
            .create_index(&[column], lancedb::index::Index::FTS(fts_index_params()))
            .execute()
            .await
            .unwrap();
    }
    table
}

#[tokio::test]
async fn bm25_matches_a_literal_snake_case_identifier() {
    use futures::TryStreamExt;
    use lancedb::query::{ExecutableQuery, QueryBase};

    let dir = tempfile::tempdir().unwrap();
    let table = build_fixture_table(dir.path()).await;

    let query = lance_index::scalar::FullTextSearchQuery::new("create_index".into())
        .with_columns(&FTS_COLUMNS.iter().map(|c| c.to_string()).collect::<Vec<_>>())
        .unwrap();

    let batches: Vec<_> = table
        .query()
        .full_text_search(query)
        .execute()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();

    let hits: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert!(
        hits > 0,
        "BM25 found nothing for a literal identifier present in the corpus. \
         The tokenizer is splitting identifiers in a way that defeats the \
         exact-match advantage hybrid retrieval exists to provide. STOP and \
         report this rather than working around it."
    );
}

/// The reason for the `code` base tokenizer, pinned as a test so a change to
/// `fts_index_params` that silently restores stemming fails here.
#[test]
fn the_configured_tokenizer_keeps_identifiers_whole() {
    let terms = |s: &str| -> Vec<String> {
        lancedb::tokenize(s, &fts_index_params())
            .unwrap()
            .into_iter()
            .map(|t| t.text)
            .collect()
    };
    assert_eq!(terms("create_index"), vec!["create_index"]);
    assert_eq!(terms("nearest_to"), vec!["nearest_to"]);
    // Prose still splits on whitespace.
    assert_eq!(terms("full text search"), vec!["full", "text", "search"]);
}
