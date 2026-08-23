# Qemer Core Retrieval Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `qemer-core` to the point where it can install a published corpus and answer a query with ranked snippets, using hybrid BM25 + vector retrieval.

**Architecture:** A corpus is downloaded as a zstd-compressed tarball of Parquet rows, checksum-verified, and built client-side into a LanceDB directory of its own with both a vector column and an FTS index. A query runs two independent searches — BM25 and vector — each of which is collapsed from rows to snippets by keeping each snippet's best-ranked row, after which the two snippet rankings are fused by reciprocal rank. Fusion is pure functions over ordered lists of ids, so it is unit-testable without a database.

**Tech Stack:** Rust 2024, `lancedb` 0.37.1, `arrow-array`/`arrow-schema` 59.2.0, `parquet`, `reqwest` 0.13, `tokio` 1.53, `sha2`, `tar`, `zstd`, `directories`.

**Spec:** [`docs/decisions.md`](../../decisions.md) — read it first. It records what is settled *and what is deliberately still open*; do not resolve an open question by picking something reasonable.

## Global Constraints

- `qemer-core` must never depend on `qemer-answer` and must never reference generation. No type, field, config key, comment, or error message in this crate may name completion, prompts, or answering. The reason is a future `qemer-mcp` that links this crate alone.
- Nothing here may assume anything about `qemer-ingest` beyond the manifest and the Parquet schema in `docs/decisions.md`. Do not write comments or errors that reason about how ingestion works.
- Embedding model and dimension are checked against the corpus **before any search runs**. A mismatch returns `CoreError::ModelMismatch`, never a degraded search.
- Qemer never installs, downloads, or launches a model runtime. It makes HTTP calls to a `llama-server` the user is already running.
- Build requires `protoc` on PATH (`apt install protobuf-compiler`). First build compiles datafusion and takes several minutes.
- Embedding: `nomic-embed-text-v1.5`, 768 dimensions.
- **Create the FTS index; never create an ANN (IVF-PQ) vector index.** Brute-force vector scan is exact and sub-millisecond at corpus sizes under ~100k vectors, and IVF-PQ would trade recall for an invisible latency win. `docs/decisions.md` records the threshold that would reopen this.
- `k` defaults to 5; each retriever over-fetches `3 * k` rows.
- RRF constant is 60.
- Commits follow Conventional Commits: `<type>: <subject>`, bulleted body when the commit has more than one distinct sub-change.

## A note on unverified APIs

Tasks 1, 8, and 10 call `lancedb` APIs that have **not been exercised against 0.37.1**. Each of those tasks opens with an explicit verification step against `docs.rs/lancedb/0.37.1`. Treat the code in those tasks as the intended shape, not as known-compiling source — if the real signature differs, follow the real signature and note the difference in the commit body. Do not guess a signature from memory.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `qemer-core/src/lib.rs` | Error type, `Result` alias, re-exports. Exists; gains variants. |
| `qemer-core/src/schema.rs` | **New.** The Arrow schema of a corpus table, and the row-level `RowHit` type. Single source of truth for column names. |
| `qemer-core/src/fuse.rs` | **New.** Pure fusion: collapse ranked rows to snippets, RRF two rankings. No I/O, no LanceDB. |
| `qemer-core/src/cache.rs` | **New.** Where corpora live on disk; enumerating and resolving them. |
| `qemer-core/src/corpus.rs` | Manifest types, download, checksum verification, unpack, table build. Exists as stubs. |
| `qemer-core/src/embed.rs` | `EmbedClient` over HTTP. Exists as a stub. |
| `qemer-core/src/search.rs` | `Snippet` and the public `search` entry point that wires the pieces together. Exists as a stub. |
| `qemer-core/tests/fixture.rs` | **New.** Shared helper that builds a small corpus table in a tempdir, used by every retrieval test. |

Fusion lives in its own file for the same reason `qemer-answer/src/prompt.rs` does: it is where the interesting edge cases are and it never touches a terminal, a network, or a database.

---

## Task 1: Corpus schema and a real test fixture

Everything downstream needs a table to search. This task builds one from hardcoded rows and, in the process, answers the FTS tokenizer question that `docs/decisions.md` flags as unverified — whether BM25 can match a literal identifier like `create_fts_index`. That answer can change the design, so it comes first.

**Files:**
- Create: `qemer-core/src/schema.rs`
- Create: `qemer-core/tests/fixture.rs`
- Modify: `qemer-core/src/lib.rs` (add `pub mod schema;`)
- Modify: `qemer-core/Cargo.toml` (dev-dependency `tempfile`)

**Interfaces:**
- Produces: `schema::corpus_schema() -> arrow_schema::SchemaRef`; column-name constants `COL_SNIPPET_ID`, `COL_KIND`, `COL_TITLE`, `COL_SOURCE_URL`, `COL_TEXT`, `COL_VECTOR`; `fixture::build_fixture_table(dir: &Path) -> lancedb::Table` (async).

- [ ] **Step 1: Verify the LanceDB API shape before writing code**

Read `docs.rs/lancedb/0.37.1` for: `lancedb::connect`, `Connection::create_table`, `Table::create_index`, `lancedb::index::Index::FTS`, and the FTS index builder's options (specifically whether a tokenizer can be configured and how columns are named). Write down the actual signatures. If they differ from the code below, follow the real ones.

- [ ] **Step 2: Add the dependencies**

`FullTextSearchQuery` lives in `lance-index`, not in `lancedb`, so it must be declared explicitly. Its version has to match the `lance-index` that `lancedb` 0.37.1 itself depends on, or the query type will be a different type than the one `full_text_search` accepts. Find the right version with `cargo tree -p qemer-core -i lance-index` after adding `lancedb`, and pin to exactly that.

```toml
lance-index = "<version matching lancedb 0.37.1 — verify, do not guess>"

[dev-dependencies]
tempfile = "3"
```

After adding it, confirm exactly one version resolves: `cargo tree -p qemer-core -i lance-index`. Two versions means `full_text_search` will reject the query you build.

- [ ] **Step 3: Write the schema module**

```rust
//! The Arrow schema of a corpus table.
//!
//! Column names live here rather than as string literals at call sites so a
//! rename is one edit and a typo is a compile error.

use arrow_schema::{DataType, Field, Schema, SchemaRef};
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
```

- [ ] **Step 4: Add the module to lib.rs**

Add `pub mod schema;` alongside the existing `pub mod corpus;` / `embed` / `search` declarations.

- [ ] **Step 5: Write the fixture helper**

Vectors here are deterministic filler — retrieval tests that care about vector ranking set them explicitly. The text is chosen so that one snippet contains a literal snake_case identifier and another describes the same thing in prose, which is exactly the case hybrid retrieval exists to handle.

```rust
//! Builds a small corpus table in a temp directory. Shared by retrieval tests.

use arrow_array::{
    FixedSizeListArray, RecordBatch, RecordBatchIterator, StringArray,
    types::Float32Type,
};
use qemer_core::schema::*;
use std::path::Path;
use std::sync::Arc;

/// (snippet_id, kind, title, source_url, text)
pub const ROWS: &[(&str, &str, &str, &str, &str)] = &[
    ("s1", "prose", "Full text search", "https://example/fts",
     "Run a keyword search over a table that has a full text search index."),
    ("s1", "code", "Full text search", "https://example/fts",
     "table.create_index(&[\"text\"], Index::FTS(Default::default())).await?;"),
    ("s2", "prose", "Vector search", "https://example/ann",
     "Find the rows whose embeddings are closest to a query vector."),
    ("s2", "code", "Vector search", "https://example/ann",
     "table.query().nearest_to(&[1.0, 2.0, 3.0])?.execute().await?;"),
    ("s3", "prose", "Connecting to a database", "https://example/connect",
     "Open a database directory. Tables are created inside it."),
];

fn batch(vectors: Vec<Vec<f32>>) -> RecordBatch {
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
                    vectors.into_iter().map(|v| Some(v.into_iter().map(Some).collect::<Vec<_>>())),
                    EMBEDDING_DIM,
                ),
            ),
        ],
    )
    .unwrap()
}

/// Deterministic filler vectors: row i is all-`i as f32 / 100.0`.
fn filler_vectors() -> Vec<Vec<f32>> {
    (0..ROWS.len())
        .map(|i| vec![i as f32 / 100.0; EMBEDDING_DIM as usize])
        .collect()
}

pub async fn build_fixture_table(dir: &Path) -> lancedb::Table {
    let db = lancedb::connect(dir.to_str().unwrap()).execute().await.unwrap();
    let schema = corpus_schema();
    let reader = RecordBatchIterator::new(
        vec![Ok(batch(filler_vectors()))].into_iter(),
        schema.clone(),
    );
    let table = db.create_table("snippets", reader).execute().await.unwrap();
    table
        .create_index(&[COL_TEXT, COL_TITLE], lancedb::index::Index::FTS(Default::default()))
        .execute()
        .await
        .unwrap();
    table
}
```

- [ ] **Step 6: Write the tokenizer probe test**

This is the load-bearing question. Append to `qemer-core/tests/fixture.rs`:

```rust
#[tokio::test]
async fn bm25_matches_a_literal_snake_case_identifier() {
    use futures::TryStreamExt;
    use lancedb::query::{ExecutableQuery, QueryBase};

    let dir = tempfile::tempdir().unwrap();
    let table = build_fixture_table(dir.path()).await;

    let batches: Vec<_> = table
        .query()
        .full_text_search(lance_index::scalar::FullTextSearchQuery::new(
            "create_index".into(),
        ))
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
```

- [ ] **Step 7: Run the probe**

Run: `cargo test -p qemer-core --test fixture -- --nocapture`
Expected: PASS. **If it fails, stop and report.** A failure means BM25 cannot match identifiers with the default tokenizer, which invalidates part of the hybrid design in `docs/decisions.md`. The fix is a design decision (custom tokenizer, or an identifier-extraction pass at index time), not an implementation detail — it belongs back with the human, not worked around here.

- [ ] **Step 8: Record the finding**

Edit `docs/decisions.md`: remove the tokenizer item from "Specifics to verify before relying on them" and add a one-line settled note stating what the tokenizer actually does, as observed.

- [ ] **Step 9: Commit**

```bash
git add qemer-core/src/schema.rs qemer-core/src/lib.rs qemer-core/tests/fixture.rs qemer-core/Cargo.toml docs/decisions.md
git commit -m "feat(core): add corpus schema and a corpus test fixture

- Define the Arrow schema and column-name constants in one place.
- Add a tempdir fixture table used by every retrieval test.
- Verify BM25 tokenizer behaviour on snake_case identifiers and record
  the observed behaviour in docs/decisions.md."
```

---

## Task 2: Snippet type cleanup

`Snippet` currently declares `language`, which the corpus contract has no column for, and documents `distance` as a cosine distance, which stops being true once results are RRF-fused. Fix both before anything is built on them.

**Files:**
- Modify: `qemer-core/src/search.rs:8-20`

**Interfaces:**
- Produces: `Snippet { library, version, snippet_id, title, description, code: Option<String>, source_url: Option<String>, score: f32 }`

- [ ] **Step 1: Rewrite the struct**

```rust
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
```

- [ ] **Step 2: Confirm it still builds**

Run: `cargo check -p qemer-core`
Expected: PASS (the `search` function body is still `todo!()`).

- [ ] **Step 3: Commit**

```bash
git add qemer-core/src/search.rs
git commit -m "refactor(core): align Snippet with the corpus contract

- Drop `language`; the Parquet schema has no such column and adding one
  would be a contract change.
- Replace `distance` with `score`, since RRF-fused results are ranks,
  not cosine distances.
- Add `snippet_id`, which fusion needs to group rows."
```

---

## Task 3: Fusion (pure)

The heart of hybrid retrieval, and the part with no I/O. Two functions: collapse a rank-ordered list of row hits into a rank-ordered list of distinct snippet ids, then fuse two such lists by reciprocal rank.

Collapsing by "first occurrence in a rank-ordered list" is deliberately equivalent to "maximum score per snippet" but avoids ever comparing a BM25 score to a cosine distance — the two have opposite polarity and incomparable scales, and never materializing them as numbers means that bug cannot be written.

**Files:**
- Create: `qemer-core/src/fuse.rs`
- Modify: `qemer-core/src/lib.rs` (add `pub mod fuse;`)

**Interfaces:**
- Produces: `fuse::collapse(ordered_snippet_ids: &[String]) -> Vec<String>`; `fuse::rrf(rankings: &[Vec<String>], k: f32) -> Vec<(String, f32)>`; `fuse::RRF_K: f32 = 60.0`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_keeps_first_occurrence_and_order() {
        let ordered = vec!["s2".into(), "s1".into(), "s2".into(), "s3".into()];
        assert_eq!(collapse(&ordered), vec!["s2", "s1", "s3"]);
    }

    #[test]
    fn collapse_of_empty_is_empty() {
        assert!(collapse(&[]).is_empty());
    }

    #[test]
    fn rrf_ranks_a_snippet_found_by_both_above_one_found_by_either() {
        let bm25 = vec!["s1".to_string(), "s2".to_string()];
        let vector = vec!["s3".to_string(), "s1".to_string()];
        let fused = rrf(&[bm25, vector], RRF_K);
        assert_eq!(fused[0].0, "s1", "s1 appears in both lists and must win");
    }

    #[test]
    fn rrf_scores_are_descending() {
        let a = vec!["x".to_string(), "y".to_string(), "z".to_string()];
        let b = vec!["z".to_string(), "y".to_string()];
        let fused = rrf(&[a, b], RRF_K);
        for pair in fused.windows(2) {
            assert!(pair[0].1 >= pair[1].1, "fused output must be sorted by score");
        }
    }

    #[test]
    fn rrf_with_one_empty_list_preserves_the_other_order() {
        let only = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let fused = rrf(&[only.clone(), vec![]], RRF_K);
        let ids: Vec<String> = fused.into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, only);
    }

    #[test]
    fn rrf_of_nothing_is_nothing() {
        assert!(rrf(&[vec![], vec![]], RRF_K).is_empty());
    }

    #[test]
    fn rrf_is_deterministic_for_tied_scores() {
        // "b" and "c" both appear once, at the same rank in different lists,
        // so their scores tie. Ties must not reorder run to run.
        let a = vec!["b".to_string()];
        let b = vec!["c".to_string()];
        let first = rrf(&[a.clone(), b.clone()], RRF_K);
        let second = rrf(&[a, b], RRF_K);
        assert_eq!(first, second);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qemer-core fuse`
Expected: FAIL to compile — `collapse`, `rrf`, and `RRF_K` do not exist.

- [ ] **Step 3: Write the implementation**

```rust
//! Rank fusion for hybrid retrieval.
//!
//! Two retrievers produce two independently ranked lists. BM25 scores are
//! unbounded and corpus-dependent; vector distances are bounded and have the
//! opposite polarity. There is no principled constant that combines them, so
//! fusion here uses ranks only and never the scores themselves.

use std::collections::HashMap;

/// The conventional reciprocal-rank-fusion damping constant.
pub const RRF_K: f32 = 60.0;

/// Collapse a rank-ordered list of row-level snippet ids into a rank-ordered
/// list of distinct ids, keeping each id's best-ranked appearance.
///
/// Because the input is already sorted best-first, keeping the first
/// occurrence *is* keeping the maximum score, without materialising a score.
pub fn collapse(ordered: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    ordered
        .iter()
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect()
}

/// Fuse rankings by reciprocal rank. Each list contributes `1 / (k + rank)`
/// per id, with `rank` 1-based. Output is sorted by score descending; ties
/// break on id so the ordering is stable across runs.
pub fn rrf(rankings: &[Vec<String>], k: f32) -> Vec<(String, f32)> {
    let mut scores: HashMap<&str, f32> = HashMap::new();
    for ranking in rankings {
        for (i, id) in ranking.iter().enumerate() {
            *scores.entry(id.as_str()).or_insert(0.0) += 1.0 / (k + (i + 1) as f32);
        }
    }
    let mut fused: Vec<(String, f32)> =
        scores.into_iter().map(|(id, s)| (id.to_string(), s)).collect();
    fused.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    fused
}
```

- [ ] **Step 4: Add the module to lib.rs**

Add `pub mod fuse;`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p qemer-core fuse`
Expected: PASS, 6 tests.

- [ ] **Step 6: Commit**

```bash
git add qemer-core/src/fuse.rs qemer-core/src/lib.rs
git commit -m "feat(core): add pure rank fusion for hybrid retrieval

- collapse() reduces rank-ordered rows to distinct snippets by first
  occurrence, which is max-score without comparing incomparable scales.
- rrf() fuses rankings by reciprocal rank with a stable tie-break."
```

---

## Task 4: Corpus cache layout

Where installed corpora live, and how they are found. Pure filesystem work, testable with a tempdir.

**Files:**
- Create: `qemer-core/src/cache.rs`
- Modify: `qemer-core/src/lib.rs` (add `pub mod cache;`)
- Modify: `qemer-core/Cargo.toml` (add `directories`)

**Interfaces:**
- Consumes: `CorpusRef` from `corpus.rs` (already defined).
- Produces: `cache::Cache::new(root: PathBuf) -> Cache`; `Cache::default_root() -> Result<PathBuf>`; `Cache::dir_for(&self, library: &str, version: &str) -> PathBuf`; `Cache::installed(&self) -> Result<Vec<Corpus>>`; `Cache::write_meta(&self, dir: &Path, r: &CorpusRef) -> Result<()>`; `Cache::read_meta(dir: &Path) -> Result<CorpusRef>`.

An installed corpus is exactly one directory, `<root>/<library>-<version>/`, containing the LanceDB data and a `corpus.json` holding the `CorpusRef` it was installed from. The metadata sits beside the data it describes so the model check can never read one corpus's stamp while searching another's.

- [ ] **Step 1: Add the dependency**

```toml
directories = "6"
```

Verify the major version actually published before pinning: `cargo add directories --dry-run -p qemer-core`. Use whatever it resolves to and record it.

- [ ] **Step 2: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::CorpusRef;

    fn a_ref() -> CorpusRef {
        CorpusRef {
            library: "lancedb".into(),
            version: "0.37.1".into(),
            url: "https://example/lancedb-0.37.1.tar.zst".into(),
            sha256: "abc".into(),
            bytes: 10,
            embedding_model: "nomic-embed-text-v1.5".into(),
            embedding_dim: 768,
            snippet_count: 3,
        }
    }

    #[test]
    fn dir_for_is_library_and_version() {
        let cache = Cache::new("/tmp/root".into());
        assert_eq!(
            cache.dir_for("lancedb", "0.37.1"),
            std::path::Path::new("/tmp/root/lancedb-0.37.1")
        );
    }

    #[test]
    fn meta_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path().to_path_buf());
        let dir = cache.dir_for("lancedb", "0.37.1");
        std::fs::create_dir_all(&dir).unwrap();
        cache.write_meta(&dir, &a_ref()).unwrap();
        let back = Cache::read_meta(&dir).unwrap();
        assert_eq!(back.library, "lancedb");
        assert_eq!(back.embedding_dim, 768);
    }

    #[test]
    fn installed_is_empty_when_root_does_not_exist() {
        let cache = Cache::new("/nonexistent/qemer-test-root".into());
        assert!(cache.installed().unwrap().is_empty());
    }

    #[test]
    fn installed_skips_directories_without_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path().to_path_buf());
        std::fs::create_dir_all(tmp.path().join("half-installed")).unwrap();
        assert!(cache.installed().unwrap().is_empty());
    }

    #[test]
    fn installed_finds_a_complete_corpus() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path().to_path_buf());
        let dir = cache.dir_for("lancedb", "0.37.1");
        std::fs::create_dir_all(&dir).unwrap();
        cache.write_meta(&dir, &a_ref()).unwrap();
        let found = cache.installed().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].reference.library, "lancedb");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p qemer-core cache`
Expected: FAIL to compile — `Cache` does not exist.

- [ ] **Step 4: Write the implementation**

```rust
//! Where installed corpora live on disk.
//!
//! An installed corpus is exactly one directory containing the database and a
//! `corpus.json` describing what it was installed from. Keeping the metadata
//! beside the data means the embedding-model check reads the stamp of the
//! corpus it is about to search, not some other one.

use crate::corpus::{Corpus, CorpusRef};
use crate::{CoreError, Result};
use std::path::{Path, PathBuf};

pub const META_FILE: &str = "corpus.json";

pub struct Cache {
    pub root: PathBuf,
}

impl Cache {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `~/.cache/qemer/corpora` on Linux, the platform equivalent elsewhere.
    pub fn default_root() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("", "", "qemer").ok_or_else(|| {
            CoreError::Io(std::io::Error::other("no home directory available"))
        })?;
        Ok(dirs.cache_dir().join("corpora"))
    }

    pub fn dir_for(&self, library: &str, version: &str) -> PathBuf {
        self.root.join(format!("{library}-{version}"))
    }

    pub fn write_meta(&self, dir: &Path, reference: &CorpusRef) -> Result<()> {
        let json = serde_json::to_vec_pretty(reference)
            .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;
        std::fs::write(dir.join(META_FILE), json)?;
        Ok(())
    }

    pub fn read_meta(dir: &Path) -> Result<CorpusRef> {
        let bytes = std::fs::read(dir.join(META_FILE))?;
        serde_json::from_slice(&bytes)
            .map_err(|e| CoreError::Io(std::io::Error::other(e)))
    }

    /// Directories without readable metadata are skipped, not errors: a
    /// half-finished install should be invisible rather than fatal.
    pub fn installed(&self) -> Result<Vec<Corpus>> {
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&self.root)? {
            let path = entry?.path();
            if !path.is_dir() {
                continue;
            }
            if let Ok(reference) = Self::read_meta(&path) {
                found.push(Corpus { reference, path });
            }
        }
        found.sort_by(|a, b| {
            (&a.reference.library, &a.reference.version)
                .cmp(&(&b.reference.library, &b.reference.version))
        });
        Ok(found)
    }
}
```

- [ ] **Step 5: Add the module to lib.rs**

Add `pub mod cache;` and `pub use cache::Cache;`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p qemer-core cache`
Expected: PASS, 5 tests.

- [ ] **Step 7: Commit**

```bash
git add qemer-core/src/cache.rs qemer-core/src/lib.rs qemer-core/Cargo.toml
git commit -m "feat(core): add the corpus cache layout

- One directory per library and version, each holding a corpus.json
  stamp beside its own data.
- installed() skips directories without metadata so a half-finished
  install is invisible rather than fatal."
```

---

## Task 5: Manifest fetching

Split the parse from the fetch so the interesting half is testable without a network.

**Files:**
- Modify: `qemer-core/src/corpus.rs` (replace the `fetch_manifest` stub)

**Interfaces:**
- Produces: `corpus::parse_manifest(bytes: &[u8]) -> Result<Manifest>`; `corpus::fetch_manifest(url: &str) -> Result<Manifest>` (async).

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "corpora": [{
        "library": "lancedb",
        "version": "0.37.1",
        "url": "https://example/lancedb-0.37.1.tar.zst",
        "sha256": "deadbeef",
        "bytes": 15728640,
        "embedding_model": "nomic-embed-text-v1.5",
        "embedding_dim": 768,
        "snippet_count": 4213
      }]
    }"#;

    #[test]
    fn parses_a_manifest() {
        let m = parse_manifest(SAMPLE.as_bytes()).unwrap();
        assert_eq!(m.corpora.len(), 1);
        assert_eq!(m.corpora[0].library, "lancedb");
        assert_eq!(m.corpora[0].embedding_dim, 768);
    }

    #[test]
    fn an_empty_manifest_is_valid() {
        let m = parse_manifest(br#"{"corpora": []}"#).unwrap();
        assert!(m.corpora.is_empty());
    }

    #[test]
    fn malformed_json_is_an_error_not_a_panic() {
        assert!(parse_manifest(b"not json").is_err());
    }

    #[test]
    fn a_missing_field_is_an_error() {
        assert!(parse_manifest(br#"{"corpora":[{"library":"x"}]}"#).is_err());
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qemer-core corpus::tests`
Expected: FAIL to compile — `parse_manifest` does not exist.

- [ ] **Step 3: Add a manifest error variant**

In `qemer-core/src/lib.rs`, add to `CoreError`:

```rust
    #[error("manifest at {url} could not be read: {reason}")]
    Manifest { url: String, reason: String },
```

- [ ] **Step 4: Write the implementation**

```rust
/// Parse a manifest. Kept separate from fetching so the parsing rules are
/// testable without a network.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest> {
    serde_json::from_slice(bytes).map_err(|e| CoreError::Manifest {
        url: "<local>".into(),
        reason: e.to_string(),
    })
}

pub async fn fetch_manifest(url: &str) -> Result<Manifest> {
    let bytes = reqwest::get(url)
        .await
        .map_err(|e| CoreError::Manifest { url: url.into(), reason: e.to_string() })?
        .error_for_status()
        .map_err(|e| CoreError::Manifest { url: url.into(), reason: e.to_string() })?
        .bytes()
        .await
        .map_err(|e| CoreError::Manifest { url: url.into(), reason: e.to_string() })?;
    parse_manifest(&bytes).map_err(|e| CoreError::Manifest {
        url: url.into(),
        reason: e.to_string(),
    })
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p qemer-core corpus::tests`
Expected: PASS, 4 tests.

- [ ] **Step 6: Commit**

```bash
git add qemer-core/src/corpus.rs qemer-core/src/lib.rs
git commit -m "feat(core): fetch and parse the corpus manifest

Splits parsing from fetching so the parse rules are covered without a
network, and adds a Manifest error variant that names the URL."
```

---

## Task 6: Checksum verification

**Files:**
- Modify: `qemer-core/src/corpus.rs`
- Modify: `qemer-core/Cargo.toml` (add `sha2`)
- Modify: `qemer-core/src/lib.rs` (add `ChecksumMismatch`)

**Interfaces:**
- Produces: `corpus::verify_sha256(bytes: &[u8], expected: &str) -> Result<()>`

- [ ] **Step 1: Add the dependency**

```toml
sha2 = "0.10"
```

- [ ] **Step 2: Add the error variant**

```rust
    #[error("corpus download failed checksum: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
```

- [ ] **Step 3: Write the failing tests**

The expected digest below is the well-known SHA-256 of the empty string; the second is of `b"abc"`. Both are standard test vectors.

```rust
    const EMPTY_SHA: &str =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const ABC_SHA: &str =
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn verifies_a_correct_checksum() {
        assert!(verify_sha256(b"abc", ABC_SHA).is_ok());
    }

    #[test]
    fn verifies_the_empty_input() {
        assert!(verify_sha256(b"", EMPTY_SHA).is_ok());
    }

    #[test]
    fn rejects_a_wrong_checksum() {
        assert!(verify_sha256(b"abc", EMPTY_SHA).is_err());
    }

    #[test]
    fn checksum_comparison_ignores_case() {
        assert!(verify_sha256(b"abc", &ABC_SHA.to_uppercase()).is_ok());
    }
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p qemer-core corpus::tests`
Expected: FAIL to compile — `verify_sha256` does not exist.

- [ ] **Step 5: Write the implementation**

```rust
/// Verify a downloaded tarball against the manifest's digest.
pub fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(CoreError::ChecksumMismatch {
            expected: expected.to_string(),
            actual,
        })
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p qemer-core corpus::tests`
Expected: PASS, 8 tests total in this module.

- [ ] **Step 7: Commit**

```bash
git add qemer-core/src/corpus.rs qemer-core/src/lib.rs qemer-core/Cargo.toml
git commit -m "feat(core): verify downloaded corpora against the manifest digest"
```

---

## Task 7: Install — unpack Parquet and build the table

The one place the corpus contract turns into a database. Download, verify, unpack the zstd tarball, read the Parquet rows, write them into a LanceDB table in a temp directory, create the FTS index, then atomically rename into place. Building elsewhere and renaming last means an interrupted install leaves no half-built corpus behind.

**Files:**
- Modify: `qemer-core/src/corpus.rs` (replace the `install` stub)
- Modify: `qemer-core/Cargo.toml` (add `tar`, `zstd`, `parquet`)

**Interfaces:**
- Consumes: `Cache`, `verify_sha256`, `schema::corpus_schema`.
- Produces: `corpus::install(cache: &Cache, reference: &CorpusRef) -> Result<Corpus>` (async); `corpus::build_table_from_parquet(db_dir: &Path, parquet: &Path) -> Result<()>` (async).

- [ ] **Step 1: Verify the API shapes before writing code**

Read `docs.rs` for: `parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder`, `zstd::stream::read::Decoder`, `tar::Archive`, and `lancedb::Connection::create_table`. Confirm how a Parquet reader yields `RecordBatch`es and what `create_table` accepts as input. Follow the real signatures.

- [ ] **Step 2: Add the dependencies**

```toml
tar = "0.4"
zstd = "0.13"
parquet = { version = "59.2.0", features = ["arrow"] }
```

Parquet must match the `arrow-*` major already pinned at 59.2.0, or the `RecordBatch` types will not be the same type. Confirm with `cargo tree -p qemer-core -i arrow-array` after adding — **more than one `arrow-array` version in that output is a build error waiting to happen.**

- [ ] **Step 3: Write the failing test**

This test builds a Parquet file from the fixture rows, installs it, and searches — proving the contract-to-database path end to end.

```rust
// qemer-core/tests/install.rs
mod fixture;

#[tokio::test]
async fn building_a_table_from_parquet_yields_searchable_rows() {
    use futures::TryStreamExt;
    use lancedb::query::ExecutableQuery;

    let tmp = tempfile::tempdir().unwrap();
    let parquet_path = tmp.path().join("corpus.parquet");
    fixture::write_fixture_parquet(&parquet_path);

    let db_dir = tmp.path().join("db");
    qemer_core::corpus::build_table_from_parquet(&db_dir, &parquet_path)
        .await
        .unwrap();

    let db = lancedb::connect(db_dir.to_str().unwrap()).execute().await.unwrap();
    let table = db.open_table("snippets").execute().await.unwrap();
    let batches: Vec<_> = table.query().execute().await.unwrap()
        .try_collect().await.unwrap();
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, fixture::ROWS.len());
}
```

- [ ] **Step 4: Add the Parquet writer to the fixture**

```rust
// append to qemer-core/tests/fixture.rs
pub fn write_fixture_parquet(path: &std::path::Path) {
    use parquet::arrow::ArrowWriter;
    let file = std::fs::File::create(path).unwrap();
    let batch = batch(filler_vectors());
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}
```

Make `batch` and `filler_vectors` `pub(crate)` or `pub` as needed for this to compile.

- [ ] **Step 5: Run the test to verify it fails**

Run: `cargo test -p qemer-core --test install`
Expected: FAIL to compile — `build_table_from_parquet` does not exist.

- [ ] **Step 6: Write the implementation**

```rust
/// Read Parquet rows and write them into a new LanceDB table with an FTS
/// index. The directory is created fresh; callers rename it into place.
pub async fn build_table_from_parquet(
    db_dir: &std::path::Path,
    parquet: &std::path::Path,
) -> Result<()> {
    use crate::schema::{COL_TEXT, COL_TITLE};
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    std::fs::create_dir_all(db_dir)?;
    let file = std::fs::File::open(parquet)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| CoreError::Io(std::io::Error::other(e)))?
        .build()
        .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;
    let arrow_schema = reader.schema();
    let batches: Vec<_> = reader.collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;

    let db = lancedb::connect(db_dir.to_str().unwrap()).execute().await?;
    let iter = arrow_array::RecordBatchIterator::new(
        batches.into_iter().map(Ok),
        arrow_schema,
    );
    let table = db.create_table("snippets", iter).execute().await?;
    table
        .create_index(&[COL_TEXT, COL_TITLE], lancedb::index::Index::FTS(Default::default()))
        .execute()
        .await?;
    Ok(())
}
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test -p qemer-core --test install`
Expected: PASS.

- [ ] **Step 8: Write the install orchestration**

```rust
/// Download, verify, unpack, build, and atomically move into the cache.
///
/// The table is built in a sibling temp directory and renamed last, so an
/// interrupted install leaves nothing behind for `Cache::installed` to find.
pub async fn install(cache: &crate::Cache, reference: &CorpusRef) -> Result<Corpus> {
    let final_dir = cache.dir_for(&reference.library, &reference.version);
    if final_dir.exists() {
        let existing = crate::Cache::read_meta(&final_dir)?;
        return Ok(Corpus { reference: existing, path: final_dir });
    }
    std::fs::create_dir_all(&cache.root)?;

    let bytes = reqwest::get(&reference.url)
        .await
        .map_err(|e| CoreError::Embed(e.to_string()))?
        .error_for_status()
        .map_err(|e| CoreError::Embed(e.to_string()))?
        .bytes()
        .await
        .map_err(|e| CoreError::Embed(e.to_string()))?;
    verify_sha256(&bytes, &reference.sha256)?;

    let staging = tempfile::tempdir_in(&cache.root)?;
    let decoder = zstd::stream::read::Decoder::new(std::io::Cursor::new(&bytes[..]))?;
    tar::Archive::new(decoder).unpack(staging.path())?;

    let parquet = staging.path().join("corpus.parquet");
    let db_dir = staging.path().join("db");
    build_table_from_parquet(&db_dir, &parquet).await?;

    std::fs::rename(&db_dir, &final_dir)?;
    cache.write_meta(&final_dir, reference)?;
    Ok(Corpus { reference: reference.clone(), path: final_dir })
}
```

`tempfile` moves from dev-dependencies to dependencies for this. Note the staging directory is created *inside* the cache root so the rename stays on one filesystem — a rename across mount points fails.

- [ ] **Step 9: Verify the whole crate still builds and tests pass**

Run: `cargo test -p qemer-core`
Expected: PASS.

- [ ] **Step 10: Commit**

```bash
git add qemer-core/src/corpus.rs qemer-core/tests/ qemer-core/Cargo.toml
git commit -m "feat(core): install corpora from Parquet into LanceDB

- Build the table and its FTS index in a staging directory inside the
  cache root, then rename into place, so an interrupted install leaves
  nothing partially built.
- Add an end-to-end test from a Parquet file to a searchable table."
```

---

## Task 8: The embedding-model guard

Must run before any search. `docs/decisions.md` calls this non-optional because a mismatch produces plausible nonsense rather than an error.

**Files:**
- Modify: `qemer-core/src/embed.rs`

**Interfaces:**
- Consumes: `CorpusRef`, `EmbedClient`.
- Produces: `EmbedClient::check_corpus(&self, reference: &CorpusRef) -> Result<()>`

- [ ] **Step 1: Write the failing tests**

```rust
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
        assert!(text.contains("nomic-embed-text-v1.5"), "error must name the client model");
    }
}
```

The dimension test matters on its own: nomic supports Matryoshka truncation, so the same model name can legitimately produce different dimensions, and only checking the name would let a 512-d corpus through to a 768-d client.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qemer-core embed`
Expected: FAIL to compile — `check_corpus` does not exist.

- [ ] **Step 3: Write the implementation**

```rust
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
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p qemer-core embed`
Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add qemer-core/src/embed.rs
git commit -m "feat(core): guard searches against embedding-model mismatch

Checks model name and dimension independently: Matryoshka truncation
means the same model name can produce different dimensions, so a name
match alone is not sufficient."
```

---

## Task 9: The embedding client

**Files:**
- Modify: `qemer-core/src/embed.rs`

**Interfaces:**
- Produces: `embed::parse_embedding(body: &[u8], expected_dim: usize) -> Result<Vec<f32>>`; `EmbedClient::embed(&self, text: &str) -> Result<Vec<f32>>` (async).

Response parsing is split out so the interesting failures — wrong dimension, empty data array, error body — are tested without a running server.

- [ ] **Step 1: Write the failing tests**

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qemer-core embed`
Expected: FAIL to compile — `parse_embedding` does not exist.

- [ ] **Step 3: Write the implementation**

```rust
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
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/v1/embeddings", self.base_url.trim_end_matches('/'));
        let response = reqwest::Client::new()
            .post(&url)
            .json(&serde_json::json!({ "input": text, "model": self.model }))
            .send()
            .await
            .map_err(|e| {
                CoreError::Embed(format!("no embedding server reachable at {url}: {e}"))
            })?;
        let body = response
            .bytes()
            .await
            .map_err(|e| CoreError::Embed(e.to_string()))?;
        parse_embedding(&body, self.dim)
    }
}
```

The unreachable-server message names the URL and says what was attempted; it does not diagnose what the user should start. That message belongs to the caller that knows what kind of server it wanted.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p qemer-core embed`
Expected: PASS, 8 tests in this module.

- [ ] **Step 5: Commit**

```bash
git add qemer-core/src/embed.rs
git commit -m "feat(core): embed queries against a running llama-server

Splits response parsing from the request so wrong dimensions, empty
data arrays, and error bodies are covered without a live server."
```

---

## Task 10: Wire up hybrid search

**Files:**
- Modify: `qemer-core/src/search.rs`
- Create: `qemer-core/tests/search.rs`

**Interfaces:**
- Consumes: everything above.
- Produces: `search::search(corpus: &Corpus, client: &EmbedClient, query: &str, k: usize) -> Result<Vec<Snippet>>` (async).

- [ ] **Step 1: Verify the query API before writing code**

Read `docs.rs/lancedb/0.37.1` for `QueryBase::full_text_search`, `Query::nearest_to`, `QueryBase::limit`, and how results expose the row's columns. Confirm whether a single-call hybrid query type exists in the Rust API — `docs/decisions.md` records that `rerank_hybrid(query, vector_results, fts_results)` implies two separate searches, but that was inferred from a signature, not tested. **We fuse with our own `fuse::rrf` either way**, because it operates on collapsed snippet rankings rather than raw row batches; do not swap in `RRFReranker` without raising it, since fusing at the row level is the double-counting bug `docs/decisions.md` explicitly rejects.

- [ ] **Step 2: Write the failing test**

```rust
// qemer-core/tests/search.rs
mod fixture;

/// The point of hybrid retrieval: a literal identifier that appears in a code
/// row must surface its snippet, even though the filler vectors carry no
/// semantic signal at all.
#[tokio::test]
async fn bm25_surfaces_a_snippet_the_vectors_cannot() {
    let dir = tempfile::tempdir().unwrap();
    let table = fixture::build_fixture_table(dir.path()).await;

    let ranked = qemer_core::search::bm25_ranking(&table, "create_index", 15).await.unwrap();
    let collapsed = qemer_core::fuse::collapse(&ranked);

    assert_eq!(collapsed.first().map(String::as_str), Some("s1"));
}

#[tokio::test]
async fn a_query_matching_nothing_returns_no_snippets() {
    let dir = tempfile::tempdir().unwrap();
    let table = fixture::build_fixture_table(dir.path()).await;

    let ranked = qemer_core::search::bm25_ranking(&table, "zzzzznotpresent", 15).await.unwrap();
    assert!(qemer_core::fuse::collapse(&ranked).is_empty());
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p qemer-core --test search`
Expected: FAIL to compile — `bm25_ranking` does not exist.

- [ ] **Step 4: Write the two rankers and the wiring**

```rust
use crate::corpus::Corpus;
use crate::embed::EmbedClient;
use crate::fuse;
use crate::schema::*;
use crate::{CoreError, Result};

use arrow_array::{Array, StringArray};
use futures::TryStreamExt;
use lancedb::query::{ExecutableQuery, QueryBase};

/// Pull the snippet_id column out of result batches, in the order the query
/// returned them. Order is the only thing fusion needs.
fn snippet_ids(batches: &[arrow_array::RecordBatch]) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    for b in batches {
        let col = b
            .column_by_name(COL_SNIPPET_ID)
            .ok_or_else(|| CoreError::Embed("result batch has no snippet_id".into()))?;
        let col = col
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| CoreError::Embed("snippet_id is not a string column".into()))?;
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
    let batches: Vec<_> = table
        .query()
        .full_text_search(lance_index::scalar::FullTextSearchQuery::new(query.into()))
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

    let db = lancedb::connect(corpus.path.to_str().unwrap()).execute().await?;
    let table = db.open_table("snippets").execute().await?;

    let over_fetch = k * 3;
    let query_vector = client.embed(query).await?;

    let lexical = fuse::collapse(&bm25_ranking(&table, query, over_fetch).await?);
    let semantic = fuse::collapse(&vector_ranking(&table, &query_vector, over_fetch).await?);

    let fused = fuse::rrf(&[lexical, semantic], fuse::RRF_K);
    let wanted: Vec<(String, f32)> = fused.into_iter().take(k).collect();

    hydrate(&table, corpus, &wanted).await
}
```

- [ ] **Step 5: Write `hydrate`**

Fusion returns ids and scores; the snippet body still has to be read back. Both rows of a snippet are needed to fill `description` and `code`.

```rust
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
    let mut parts: std::collections::HashMap<String, (String, Option<String>, String, Option<String>)> =
        std::collections::HashMap::new();

    for b in &batches {
        let get = |name: &str| -> Result<StringArray> {
            b.column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned())
                .ok_or_else(|| CoreError::Embed(format!("missing column {name}")))
        };
        let (ids, kinds, titles, urls, texts) = (
            get(COL_SNIPPET_ID)?, get(COL_KIND)?, get(COL_TITLE)?,
            get(COL_SOURCE_URL)?, get(COL_TEXT)?,
        );
        for i in 0..ids.len() {
            let entry = parts.entry(ids.value(i).to_string()).or_insert_with(|| {
                (
                    titles.value(i).to_string(),
                    if urls.is_null(i) { None } else { Some(urls.value(i).to_string()) },
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
            parts.remove(id).map(|(title, source_url, description, code)| Snippet {
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
```

The `IN (...)` predicate is built from snippet ids that came out of the corpus itself, not from user input, so it cannot carry a query string into the filter. Keep it that way — if ids ever become user-supplied, this needs escaping.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p qemer-core --test search`
Expected: PASS, 2 tests.

- [ ] **Step 7: Run the whole suite**

Run: `cargo test -p qemer-core`
Expected: PASS, everything.

- [ ] **Step 8: Check the boundary held**

Run: `grep -rniE "generat|prompt|answer|completion|llm" qemer-core/src/`
Expected: no hits describing generation. `qemer-core` must not reference it — see Global Constraints. If there are hits, fix them before committing.

- [ ] **Step 9: Commit**

```bash
git add qemer-core/src/search.rs qemer-core/tests/search.rs
git commit -m "feat(core): hybrid BM25 and vector search with RRF fusion

- Each retriever over-fetches 3k rows and is collapsed to snippets
  independently, so neither can vote twice for one snippet.
- Fuse the two snippet rankings with fuse::rrf rather than at row
  level, per docs/decisions.md.
- Hydrate the chosen snippets back into prose and code, preserving
  fused order.
- Check the embedding model before any query runs."
```

---

## What this plan does not cover

Deliberately out of scope, each needing its own plan:

- **`qemer-answer`** — prompt assembly, budgeting, streamed generation.
- **`qemer-tui`** — the terminal UI, config loading, blocking-with-Esc-abort generation.
- **Corpus browsing and install UX** — still an open question in `docs/decisions.md`; do not design it here.
- **Version selection and cache eviction** — open questions. `install` currently treats an existing directory as already-installed and returns it; that is the minimum, not a decision about upgrades.

## Open questions this plan must not answer on its own

If executing this plan appears to require resolving any of these, **stop and ask**:

- What the failure message says when `llama-server` is unreachable, beyond naming the URL.
- Whether a newer corpus version should be preferred, offered, or ignored.
- When a cached corpus is evicted.
