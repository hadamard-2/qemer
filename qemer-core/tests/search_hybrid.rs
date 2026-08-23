//! Exercises `search::search` and its `hydrate` step end to end: a real
//! fixture table, a real HTTP round trip to a stub embedding server, real
//! BM25 and vector queries, real fusion, real snippet assembly. The only
//! thing faked is what the embedding model would have said.

mod embed_stub;
mod fixture;

use qemer_core::corpus::{Corpus, CorpusRef};
use qemer_core::embed::EmbedClient;
use qemer_core::schema::EMBEDDING_DIM;

fn a_ref() -> CorpusRef {
    CorpusRef {
        library: "lancedb".into(),
        version: "0.37.1".into(),
        url: String::new(),
        sha256: String::new(),
        bytes: 0,
        embedding_model: "nomic-embed-text-v1.5".into(),
        embedding_dim: EMBEDDING_DIM as usize,
        snippet_count: fixture::ROWS.len(),
    }
}

#[tokio::test]
async fn search_fuses_bm25_and_vector_results_into_hydrated_snippets() {
    let dir = tempfile::tempdir().unwrap();
    fixture::build_fixture_table(dir.path()).await;
    let corpus = Corpus {
        reference: a_ref(),
        path: dir.path().to_path_buf(),
    };

    // Fixture filler vectors are uniform `i / 100.0`; row index 2 is s2's
    // prose row, so a query vector of all-0.02 lands exactly on it.
    let query_vector = vec![0.02f32; EMBEDDING_DIM as usize];
    let base_url = embed_stub::start(query_vector).await;
    let client = EmbedClient {
        base_url,
        model: "nomic-embed-text-v1.5".into(),
        dim: EMBEDDING_DIM as usize,
    };

    // "create_index" is the term the Task 1 tokenizer probe matches literally
    // in s1's code row; nothing in s2's text contains it, so s2 can only
    // surface via the vector side.
    let results = qemer_core::search::search(&corpus, &client, "create_index", 5)
        .await
        .unwrap();

    let ids: Vec<&str> = results.iter().map(|s| s.snippet_id.as_str()).collect();
    assert!(
        ids.contains(&"s1"),
        "BM25's exact identifier match must surface s1: {ids:?}"
    );
    assert!(
        ids.contains(&"s2"),
        "the mocked embedding must surface s2 through the vector side: {ids:?}"
    );

    let s1 = results.iter().find(|s| s.snippet_id == "s1").unwrap();
    assert_eq!(s1.library, "lancedb");
    assert_eq!(s1.version, "0.37.1");
    assert_eq!(s1.title, "Full text search");
    assert!(s1.description.contains("keyword search"));
    assert!(s1.code.as_deref().unwrap().contains("create_index"));
}

#[tokio::test]
async fn search_refuses_a_mismatched_embedding_model_before_any_search_runs() {
    let dir = tempfile::tempdir().unwrap();
    fixture::build_fixture_table(dir.path()).await;
    let corpus = Corpus {
        reference: a_ref(),
        path: dir.path().to_path_buf(),
    };

    // The stub is started but must never be reached: the model check has to
    // fail before `embed` makes a request.
    let base_url = embed_stub::start(vec![0.0; EMBEDDING_DIM as usize]).await;
    let client = EmbedClient {
        base_url,
        model: "a-different-model".into(),
        dim: EMBEDDING_DIM as usize,
    };

    let err = qemer_core::search::search(&corpus, &client, "create_index", 5)
        .await
        .unwrap_err();
    assert!(matches!(err, qemer_core::CoreError::ModelMismatch { .. }));
}
