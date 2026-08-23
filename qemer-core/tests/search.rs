mod fixture;

/// The point of hybrid retrieval: a literal identifier that appears in a code
/// row must surface its snippet, even though the filler vectors carry no
/// semantic signal at all.
#[tokio::test]
async fn bm25_surfaces_a_snippet_the_vectors_cannot() {
    let dir = tempfile::tempdir().unwrap();
    let table = fixture::build_fixture_table(dir.path()).await;

    let ranked = qemer_core::search::bm25_ranking(&table, "create_index", 15)
        .await
        .unwrap();
    let collapsed = qemer_core::fuse::collapse(&ranked);

    assert_eq!(collapsed.first().map(String::as_str), Some("s1"));
}

#[tokio::test]
async fn a_query_matching_nothing_returns_no_snippets() {
    let dir = tempfile::tempdir().unwrap();
    let table = fixture::build_fixture_table(dir.path()).await;

    let ranked = qemer_core::search::bm25_ranking(&table, "zzzzznotpresent", 15)
        .await
        .unwrap();
    assert!(qemer_core::fuse::collapse(&ranked).is_empty());
}

/// Exercises the vector path against a real table. The fixture's filler
/// vectors carry no meaning, so this asserts the shape of the result rather
/// than its relevance: every row ranked, nothing dropped.
#[tokio::test]
async fn vector_ranking_returns_rows_best_first() {
    let dir = tempfile::tempdir().unwrap();
    let table = fixture::build_fixture_table(dir.path()).await;

    // Row 0's filler vector exactly, so row 0 must rank first.
    let query = vec![0.0f32; qemer_core::schema::EMBEDDING_DIM as usize];
    let ranked = qemer_core::search::vector_ranking(&table, &query, 15)
        .await
        .unwrap();

    assert_eq!(ranked.len(), fixture::ROWS.len());
    assert_eq!(ranked.first().map(String::as_str), Some("s1"));
}
