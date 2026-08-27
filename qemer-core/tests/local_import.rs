mod fixture;

use std::path::{Path, PathBuf};

fn write_artifact(dir: &Path) -> (PathBuf, String, u64) {
    let parquet = dir.join("corpus.parquet");
    fixture::write_fixture_parquet(&parquet);

    let artifact = dir.join("numpy-2.3.0.tar.zst");
    let file = std::fs::File::create(&artifact).unwrap();
    let encoder = zstd::stream::write::Encoder::new(file, 0).unwrap();
    let mut tar = tar::Builder::new(encoder);
    tar.append_path_with_name(&parquet, "corpus.parquet")
        .unwrap();
    let encoder = tar.into_inner().unwrap();
    encoder.finish().unwrap();

    let bytes = std::fs::read(&artifact).unwrap();
    let digest = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(&bytes))
    };
    (artifact, digest, bytes.len() as u64)
}

fn write_manifest(dir: &Path, artifact: &str, sha256: &str, bytes: u64) -> PathBuf {
    let manifest = serde_json::json!({
        "corpora": [{
            "library": "numpy",
            "version": "2.3.0",
            "url": artifact,
            "sha256": sha256,
            "bytes": bytes,
            "embedding_model": "nomic-embed-text-v1.5",
            "embedding_dim": 768,
            "snippet_count": 3
        }]
    });
    let path = dir.join("manifest.json");
    std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    path
}

#[tokio::test]
async fn installs_a_local_manifest_artifact_into_the_cache() {
    let source = tempfile::tempdir().unwrap();
    let (_, sha256, bytes) = write_artifact(source.path());
    let manifest_path = write_manifest(source.path(), "numpy-2.3.0.tar.zst", &sha256, bytes);
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = qemer_core::Cache::new(cache_dir.path().to_path_buf());

    let manifest = qemer_core::corpus::load_manifest(manifest_path.to_str().unwrap())
        .await
        .unwrap();
    let reference = qemer_core::corpus::find_corpus(manifest, "numpy", "2.3.0").unwrap();
    let installed = qemer_core::corpus::install(&cache, &reference)
        .await
        .unwrap();

    assert!(installed.path.join("corpus.json").is_file());
    assert_eq!(cache.installed().unwrap()[0].reference.library, "numpy");
}

#[tokio::test]
async fn reports_the_missing_relative_artifact_path() {
    let source = tempfile::tempdir().unwrap();
    let manifest_path = write_manifest(
        source.path(),
        "missing.tar.zst",
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        0,
    );
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = qemer_core::Cache::new(cache_dir.path().to_path_buf());

    let manifest = qemer_core::corpus::load_manifest(manifest_path.to_str().unwrap())
        .await
        .unwrap();
    let reference = qemer_core::corpus::find_corpus(manifest, "numpy", "2.3.0").unwrap();
    let error = match qemer_core::corpus::install(&cache, &reference).await {
        Err(error) => error,
        Ok(_) => panic!("install unexpectedly succeeded"),
    };

    assert!(matches!(
        error,
        qemer_core::CoreError::FileRead { path, .. } if path == source.path().join("missing.tar.zst")
    ));
}

#[tokio::test]
async fn rejects_a_local_artifact_with_the_wrong_advertised_size_before_unpacking() {
    let source = tempfile::tempdir().unwrap();
    let (_, sha256, bytes) = write_artifact(source.path());
    let manifest_path = write_manifest(source.path(), "numpy-2.3.0.tar.zst", &sha256, bytes + 1);
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = qemer_core::Cache::new(cache_dir.path().to_path_buf());

    let manifest = qemer_core::corpus::load_manifest(manifest_path.to_str().unwrap())
        .await
        .unwrap();
    let reference = qemer_core::corpus::find_corpus(manifest, "numpy", "2.3.0").unwrap();
    let error = match qemer_core::corpus::install(&cache, &reference).await {
        Err(error) => error,
        Ok(_) => panic!("install unexpectedly succeeded"),
    };

    assert!(matches!(
        error,
        qemer_core::CoreError::SizeMismatch { expected, actual }
            if expected == bytes + 1 && actual == bytes
    ));
    assert!(!cache.dir_for("numpy", "2.3.0").exists());
}
