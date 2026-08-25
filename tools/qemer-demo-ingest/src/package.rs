use std::{path::Path, sync::Arc};

use arrow_array::{
    RecordBatch,
    builder::{FixedSizeListBuilder, Float32Builder, StringBuilder},
};
use arrow_schema::{DataType, Field, Schema, SchemaRef};
use parquet::arrow::ArrowWriter;
use sha2::{Digest, Sha256};

use crate::{IngestError, embed::EmbeddedUnit};

#[derive(Debug, serde::Serialize)]
pub struct Manifest {
    pub corpora: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ManifestEntry {
    pub library: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub snippet_count: usize,
}

#[derive(Debug, Clone)]
pub struct CorpusIdentity {
    pub library: String,
    pub version: String,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub asset_base_url: String,
}

pub fn corpus_schema(dimension: i32) -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("snippet_id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("source_url", DataType::Utf8, true),
        Field::new("text", DataType::Utf8, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, true)),
                dimension,
            ),
            false,
        ),
    ]))
}

pub fn write_parquet(
    path: &Path,
    rows: &[EmbeddedUnit],
    dimension: i32,
) -> Result<(), IngestError> {
    let expected_dimension = usize::try_from(dimension)
        .map_err(|_| IngestError::Parquet(format!("invalid vector dimension {dimension}")))?;
    for row in rows {
        if row.vector.len() != expected_dimension {
            return Err(IngestError::Parquet(format!(
                "snippet {} has vector width {}; expected {expected_dimension}",
                row.unit.snippet_id,
                row.vector.len()
            )));
        }
    }

    let mut snippet_ids = StringBuilder::new();
    let mut kinds = StringBuilder::new();
    let mut titles = StringBuilder::new();
    let mut source_urls = StringBuilder::new();
    let mut texts = StringBuilder::new();
    let mut vectors = FixedSizeListBuilder::new(Float32Builder::new(), dimension);
    for row in rows {
        snippet_ids.append_value(&row.unit.snippet_id);
        kinds.append_value(row.unit.kind);
        titles.append_value(&row.unit.title);
        source_urls.append_value(&row.unit.source_url);
        texts.append_value(&row.unit.text);
        vectors.values().append_slice(&row.vector);
        vectors.append(true);
    }

    let schema = corpus_schema(dimension);
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(snippet_ids.finish()),
            Arc::new(kinds.finish()),
            Arc::new(titles.finish()),
            Arc::new(source_urls.finish()),
            Arc::new(texts.finish()),
            Arc::new(vectors.finish()),
        ],
    )
    .map_err(|error| IngestError::Parquet(error.to_string()))?;
    let file = std::fs::File::create(path)?;
    let mut writer = ArrowWriter::try_new(file, schema, None)
        .map_err(|error| IngestError::Parquet(error.to_string()))?;
    writer
        .write(&batch)
        .map_err(|error| IngestError::Parquet(error.to_string()))?;
    writer
        .close()
        .map_err(|error| IngestError::Parquet(error.to_string()))?;
    Ok(())
}

pub fn write_archive(parquet: &Path, archive: &Path) -> Result<(), IngestError> {
    let encoder = zstd::stream::write::Encoder::new(std::fs::File::create(archive)?, 19)
        .map_err(|error| IngestError::Archive(error.to_string()))?;
    let mut tar = tar::Builder::new(encoder);
    tar.append_path_with_name(parquet, "corpus.parquet")
        .map_err(|error| IngestError::Archive(error.to_string()))?;
    let encoder = tar
        .into_inner()
        .map_err(|error| IngestError::Archive(error.to_string()))?;
    encoder
        .finish()
        .map_err(|error| IngestError::Archive(error.to_string()))?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String, IngestError> {
    let bytes = std::fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub fn stage_corpus(
    output_dir: &Path,
    identity: &CorpusIdentity,
    rows: &[EmbeddedUnit],
) -> Result<ManifestEntry, IngestError> {
    let staging = tempfile::tempdir()?;
    let parquet = staging.path().join("corpus.parquet");
    let dimension = i32::try_from(identity.embedding_dim).map_err(|_| {
        IngestError::Parquet(format!(
            "embedding dimension {} exceeds Arrow's supported range",
            identity.embedding_dim
        ))
    })?;
    write_parquet(&parquet, rows, dimension)?;

    let archive_name = format!("{}-{}.tar.zst", identity.library, identity.version);
    let archive = output_dir.join(&archive_name);
    write_archive(&parquet, &archive)?;
    let bytes = std::fs::metadata(&archive)?.len();
    let sha256 = sha256_file(&archive)?;
    let asset_base_url = identity.asset_base_url.trim_end_matches('/');

    Ok(ManifestEntry {
        library: identity.library.clone(),
        version: identity.version.clone(),
        url: format!("{asset_base_url}/{archive_name}"),
        sha256,
        bytes,
        embedding_model: identity.embedding_model.clone(),
        embedding_dim: identity.embedding_dim,
        snippet_count: rows.len(),
    })
}

pub fn write_manifest(output_dir: &Path, entries: &[ManifestEntry]) -> Result<(), IngestError> {
    let manifest = Manifest {
        corpora: entries.to_vec(),
    };
    let bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| IngestError::Manifest(error.to_string()))?;
    std::fs::write(output_dir.join("manifest.json"), bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        embed::EmbeddedUnit,
        package::{CorpusIdentity, stage_corpus, write_archive, write_manifest, write_parquet},
        snapshot::TextUnit,
    };
    use arrow_array::RecordBatchReader;
    use arrow_schema::{DataType, Field, Schema};
    use sha2::{Digest, Sha256};
    use std::sync::Arc;

    fn rows() -> Vec<EmbeddedUnit> {
        vec![
            EmbeddedUnit {
                unit: TextUnit {
                    snippet_id: "numpy-2026-08-24-000001".into(),
                    kind: "prose",
                    title: "Example".into(),
                    source_url: "https://example.test/prose".into(),
                    text: "Prose text".into(),
                },
                vector: vec![1.0, 2.0, 3.0],
            },
            EmbeddedUnit {
                unit: TextUnit {
                    snippet_id: "numpy-2026-08-24-000001".into(),
                    kind: "code",
                    title: "Example".into(),
                    source_url: "https://example.test/code".into(),
                    text: "print('code')".into(),
                },
                vector: vec![4.0, 5.0, 6.0],
            },
        ]
    }

    #[test]
    fn parquet_uses_the_qemer_row_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corpus.parquet");
        write_parquet(&path, &rows(), 3).unwrap();
        let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(
            std::fs::File::open(path).unwrap(),
        )
        .unwrap()
        .build()
        .unwrap();
        let schema = reader.schema();
        assert_eq!(
            schema.as_ref(),
            &Schema::new(vec![
                Field::new("snippet_id", DataType::Utf8, false),
                Field::new("kind", DataType::Utf8, false),
                Field::new("title", DataType::Utf8, false),
                Field::new("source_url", DataType::Utf8, true),
                Field::new("text", DataType::Utf8, false),
                Field::new(
                    "vector",
                    DataType::FixedSizeList(
                        Arc::new(Field::new("item", DataType::Float32, true)),
                        3,
                    ),
                    false,
                ),
            ])
        );
    }

    #[test]
    fn archive_places_parquet_at_the_installers_expected_path() {
        let dir = tempfile::tempdir().unwrap();
        let parquet = dir.path().join("corpus.parquet");
        write_parquet(&parquet, &rows(), 3).unwrap();
        let archive = dir.path().join("numpy-2026-08-24.tar.zst");
        write_archive(&parquet, &archive).unwrap();
        let decoder =
            zstd::stream::read::Decoder::new(std::fs::File::open(archive).unwrap()).unwrap();
        let names = tar::Archive::new(decoder)
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![std::path::PathBuf::from("corpus.parquet")]);
    }

    #[test]
    fn parquet_rejects_a_vector_with_the_wrong_width() {
        let dir = tempfile::tempdir().unwrap();
        let mut rows = rows();
        rows[0].vector.pop();

        let error = write_parquet(&dir.path().join("corpus.parquet"), &rows, 3).unwrap_err();
        assert!(error.to_string().contains("expected 3"));
    }

    #[test]
    fn manifest_records_the_completed_release_asset() {
        let dir = tempfile::tempdir().unwrap();
        let identity = CorpusIdentity {
            library: "numpy".into(),
            version: "2026-08-24".into(),
            embedding_model: "nomic-embed-text-v1.5".into(),
            embedding_dim: 3,
            asset_base_url:
                "https://github.com/example/qemer-corpora/releases/download/demo-2026-08-25".into(),
        };
        let entry = stage_corpus(dir.path(), &identity, &rows()).unwrap();
        write_manifest(dir.path(), &[entry]).unwrap();

        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("manifest.json")).unwrap())
                .unwrap();
        let entry = &manifest["corpora"][0];
        let archive = dir.path().join("numpy-2026-08-24.tar.zst");
        let independently_computed_sha256 =
            format!("{:x}", Sha256::digest(std::fs::read(&archive).unwrap()));
        assert_eq!(
            entry["url"],
            "https://github.com/example/qemer-corpora/releases/download/demo-2026-08-25/numpy-2026-08-24.tar.zst"
        );
        assert_eq!(entry["sha256"], independently_computed_sha256);
        assert!(entry["bytes"].as_u64().unwrap() > 0);
        assert_eq!(entry["embedding_model"], "nomic-embed-text-v1.5");
        assert_eq!(entry["embedding_dim"], 3);
        assert_eq!(entry["snippet_count"], 2);
    }
}
