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

    let db = lancedb::connect(db_dir.to_str().unwrap())
        .execute()
        .await
        .unwrap();
    let table = db.open_table("snippets").execute().await.unwrap();
    let batches: Vec<_> = table
        .query()
        .execute()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(rows, fixture::ROWS.len());
}
