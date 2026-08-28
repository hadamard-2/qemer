//! Hybrid BM25 and vector search over an installed corpus.

use crate::corpus::Corpus;
use crate::embed::EmbedClient;
use crate::fuse;
use crate::schema::*;
use crate::{CoreError, Result};

use futures::TryStreamExt;
use lancedb::arrow::arrow_array::{Array, RecordBatch, StringArray};
use lancedb::query::{ExecutableQuery, QueryBase};

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

/// Pull the snippet_id column out of result batches, in the order the query
/// returned them. Order is the only thing fusion needs.
fn snippet_ids(batches: &[RecordBatch]) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for b in batches {
        let col = b.column_by_name(COL_SNIPPET_ID).ok_or_else(|| {
            CoreError::Db(lancedb::Error::Other {
                message: "result batch has no snippet_id column".into(),
                source: None,
            })
        })?;
        let col = col.as_any().downcast_ref::<StringArray>().ok_or_else(|| {
            CoreError::Db(lancedb::Error::Other {
                message: "snippet_id is not a string column".into(),
                source: None,
            })
        })?;
        for i in 0..col.len() {
            ids.push(col.value(i).to_string());
        }
    }
    Ok(ids)
}

/// BM25 ranking of rows, best first.
pub async fn bm25_ranking(
    table: &lancedb::Table,
    query: &str,
    limit: usize,
) -> Result<Vec<String>> {
    let fts = lance_index::scalar::FullTextSearchQuery::new(query.into())
        .with_columns(
            &FTS_COLUMNS
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>(),
        )
        .map_err(|e| {
            CoreError::Db(lancedb::Error::Other {
                message: e.to_string(),
                source: None,
            })
        })?;
    let batches: Vec<_> = table
        .query()
        .full_text_search(fts)
        .limit(limit)
        .execute()
        .await?
        .try_collect()
        .await?;
    snippet_ids(&batches)
}

/// Vector ranking of rows, best first.
pub async fn vector_ranking(
    table: &lancedb::Table,
    query_vector: &[f32],
    limit: usize,
) -> Result<Vec<String>> {
    let batches: Vec<_> = table
        .query()
        .nearest_to(query_vector)?
        .limit(limit)
        .execute()
        .await?
        .try_collect()
        .await?;
    snippet_ids(&batches)
}

/// Search a single library's corpus. Library scoping is deliberate: routing a
/// query across libraries is a separate problem we are not solving yet.
pub async fn search(
    corpus: &Corpus,
    client: &EmbedClient,
    query: &str,
    k: usize,
) -> Result<Vec<Snippet>> {
    // Non-optional, and first: a mismatched corpus returns plausible
    // nonsense rather than an error.
    client.check_corpus(&corpus.reference)?;

    let db = lancedb::connect(corpus.path.to_str().unwrap())
        .execute()
        .await?;
    let table = db.open_table("snippets").execute().await?;

    let over_fetch = k * 3;
    let query_vector = client.embed(query).await?;

    let lexical = fuse::collapse(&bm25_ranking(&table, query, over_fetch).await?);
    let semantic = fuse::collapse(&vector_ranking(&table, &query_vector, over_fetch).await?);

    let fused = fuse::rrf(&[lexical, semantic], fuse::RRF_K);
    let wanted: Vec<(String, f32)> = fused.into_iter().take(k).collect();

    hydrate(&table, corpus, &wanted).await
}

/// Read back the prose and code rows for each chosen snippet and assemble
/// them, preserving the fused order.
async fn hydrate(
    table: &lancedb::Table,
    corpus: &Corpus,
    wanted: &[(String, f32)],
) -> Result<Vec<Snippet>> {
    if wanted.is_empty() {
        return Ok(Vec::new());
    }
    // Built from snippet ids that came out of the corpus itself, never from
    // user input, so it cannot carry a query string into the filter. Keep it
    // that way — if ids ever become user-supplied, this needs escaping.
    let quoted: Vec<String> = wanted.iter().map(|(id, _)| format!("'{id}'")).collect();
    let predicate = format!("{COL_SNIPPET_ID} IN ({})", quoted.join(", "));

    let batches: Vec<_> = table
        .query()
        .only_if(predicate)
        .execute()
        .await?
        .try_collect()
        .await?;

    // snippet_id -> (title, source_url, prose, code)
    let mut parts: std::collections::HashMap<
        String,
        (String, Option<String>, String, Option<String>),
    > = std::collections::HashMap::new();

    for b in &batches {
        let get = |name: &str| -> Result<StringArray> {
            b.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned())
                .ok_or_else(|| {
                    CoreError::Db(lancedb::Error::Other {
                        message: format!("missing or non-string column {name}"),
                        source: None,
                    })
                })
        };
        let (ids, kinds, titles, urls, texts) = (
            get(COL_SNIPPET_ID)?,
            get(COL_KIND)?,
            get(COL_TITLE)?,
            get(COL_SOURCE_URL)?,
            get(COL_TEXT)?,
        );
        for i in 0..ids.len() {
            let entry = parts.entry(ids.value(i).to_string()).or_insert_with(|| {
                (
                    titles.value(i).to_string(),
                    if urls.is_null(i) {
                        None
                    } else {
                        Some(urls.value(i).to_string())
                    },
                    String::new(),
                    None,
                )
            });
            match kinds.value(i) {
                "code" => entry.3 = Some(texts.value(i).to_string()),
                _ => entry.2 = texts.value(i).to_string(),
            }
        }
    }

    Ok(wanted
        .iter()
        .filter_map(|(id, score)| {
            parts
                .remove(id)
                .map(|(title, source_url, description, code)| Snippet {
                    library: corpus.reference.library.clone(),
                    version: corpus.reference.version.clone(),
                    snippet_id: id.clone(),
                    title,
                    description,
                    code,
                    source_url,
                    score: *score,
                })
        })
        .collect())
}
