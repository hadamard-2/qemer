use std::{path::Path, process::Command, sync::Arc};

use lancedb::arrow::arrow_array::{
    FixedSizeListArray, RecordBatch, StringArray, types::Float32Type,
};
use qemer_core::schema::{EMBEDDING_DIM, corpus_schema};

fn write_fixture_parquet(path: &Path) {
    use parquet::arrow::ArrowWriter;

    let batch = RecordBatch::try_new(
        corpus_schema(),
        vec![
            Arc::new(StringArray::from_iter_values(["numpy-install"])),
            Arc::new(StringArray::from_iter_values(["prose"])),
            Arc::new(StringArray::from_iter_values(["Installing NumPy"])),
            Arc::new(StringArray::from_iter_values(["https://numpy.org"])),
            Arc::new(StringArray::from_iter_values(["Install NumPy locally."])),
            Arc::new(
                FixedSizeListArray::from_iter_primitive::<Float32Type, _, _>(
                    [Some(
                        std::iter::repeat_n(Some(0.0), EMBEDDING_DIM as usize).collect::<Vec<_>>(),
                    )],
                    EMBEDDING_DIM,
                ),
            ),
        ],
    )
    .unwrap();

    let file = std::fs::File::create(path).unwrap();
    let mut writer = ArrowWriter::try_new(file, batch.schema(), None).unwrap();
    writer.write(&batch).unwrap();
    writer.close().unwrap();
}

fn write_manifest_fixture(dir: &Path) -> std::path::PathBuf {
    let parquet = dir.join("corpus.parquet");
    write_fixture_parquet(&parquet);

    let artifact = dir.join("numpy-2.3.0.tar.zst");
    let file = std::fs::File::create(&artifact).unwrap();
    let encoder = zstd::stream::write::Encoder::new(file, 0).unwrap();
    let mut tar = tar::Builder::new(encoder);
    tar.append_path_with_name(&parquet, "corpus.parquet")
        .unwrap();
    tar.into_inner().unwrap().finish().unwrap();

    let artifact_bytes = std::fs::read(&artifact).unwrap();
    let sha256 = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(&artifact_bytes))
    };
    let manifest = serde_json::json!({
        "corpora": [{
            "library": "numpy",
            "version": "2.3.0",
            "url": "numpy-2.3.0.tar.zst",
            "sha256": sha256,
            "bytes": artifact_bytes.len(),
            "embedding_model": "nomic-embed-text-v1.5",
            "embedding_dim": 768,
            "snippet_count": 1
        }]
    });
    let manifest_path = dir.join("manifest.json");
    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    manifest_path
}

fn qemer(cache_home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_qemer"));
    command.env("XDG_CACHE_HOME", cache_home);
    command
}

#[test]
fn local_manifest_commands_discover_install_and_list_a_corpus() {
    let fixture = tempfile::tempdir().unwrap();
    let manifest = write_manifest_fixture(fixture.path());
    let cache_home = tempfile::tempdir().unwrap();

    let available = qemer(cache_home.path())
        .args(["available", "--manifest"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(
        available.status.success(),
        "available failed: {}",
        String::from_utf8_lossy(&available.stderr)
    );
    assert!(
        String::from_utf8_lossy(&available.stdout).contains("numpy@2.3.0"),
        "available did not list the fixture corpus: {}",
        String::from_utf8_lossy(&available.stdout)
    );

    let install = qemer(cache_home.path())
        .args(["install", "numpy@2.3.0", "--manifest"])
        .arg(&manifest)
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "install failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let list = qemer(cache_home.path()).arg("list").output().unwrap();
    assert!(
        list.status.success(),
        "list failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert!(
        String::from_utf8_lossy(&list.stdout).contains("numpy 2.3.0"),
        "list did not show the installed fixture corpus: {}",
        String::from_utf8_lossy(&list.stdout)
    );
}
