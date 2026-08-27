//! Discovery, download, and verification of prebuilt corpora.
//!
//! Corpora are built and published by `qemer-ingest`, a separate repository.
//! The contract between the two is this manifest plus the tarball layout;
//! nothing here should assume anything else about how ingestion works.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use crate::{CoreError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ManifestSource {
    File(PathBuf),
    Https(url::Url),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ArtifactSource {
    File(PathBuf),
    Https(url::Url),
}

impl ManifestSource {
    fn parse(source: &str) -> Result<Self> {
        match url::Url::parse(source) {
            Ok(url) if url.scheme() == "https" && url.has_host() => Ok(Self::Https(url)),
            Ok(url) => Err(manifest_error(
                source,
                format!("unsupported manifest source scheme `{}`", url.scheme()),
            )),
            Err(url::ParseError::RelativeUrlWithoutBase) => std::path::absolute(source)
                .map(Self::File)
                .map_err(|e| manifest_error(source, e.to_string())),
            Err(e) => Err(manifest_error(source, e.to_string())),
        }
    }

    fn resolve_artifact(&self, artifact: &str) -> Result<ArtifactSource> {
        match url::Url::parse(artifact) {
            Ok(url) if url.scheme() == "https" && url.has_host() => Ok(ArtifactSource::Https(url)),
            Ok(url) => Err(manifest_error(
                artifact,
                format!("unsupported artifact source scheme `{}`", url.scheme()),
            )),
            Err(url::ParseError::RelativeUrlWithoutBase) => {
                let path = Path::new(artifact);
                if path.is_absolute() {
                    return Ok(ArtifactSource::File(path.to_path_buf()));
                }

                match self {
                    Self::File(manifest) => Ok(ArtifactSource::File(
                        manifest
                            .parent()
                            .unwrap_or_else(|| Path::new(""))
                            .join(path),
                    )),
                    Self::Https(manifest) => manifest
                        .join(artifact)
                        .map(ArtifactSource::Https)
                        .map_err(|e| manifest_error(artifact, e.to_string())),
                }
            }
            Err(e) => Err(manifest_error(artifact, e.to_string())),
        }
    }
}

impl std::fmt::Display for ArtifactSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(path) => write!(f, "{}", path.display()),
            Self::Https(url) => write!(f, "{url}"),
        }
    }
}

fn manifest_error(source: &str, reason: impl Into<String>) -> CoreError {
    CoreError::Manifest {
        url: source.into(),
        reason: reason.into(),
    }
}

/// Fetched from a known URL; lists what is available to download.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub corpora: Vec<CorpusRef>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CorpusRef {
    pub library: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub snippet_count: usize,
}

/// An installed, verified corpus on local disk.
pub struct Corpus {
    pub reference: CorpusRef,
    pub path: std::path::PathBuf,
}

/// Parse a manifest. Kept separate from fetching so the parsing rules are
/// testable without a network.
pub fn parse_manifest(bytes: &[u8]) -> Result<Manifest> {
    let manifest = serde_json::from_slice(bytes).map_err(|e| CoreError::Manifest {
        url: "<local>".into(),
        reason: e.to_string(),
    })?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

fn validate_manifest(manifest: &Manifest) -> Result<()> {
    let mut identities = HashSet::new();
    for corpus in &manifest.corpora {
        if !identities.insert((&corpus.library, &corpus.version)) {
            return Err(manifest_error(
                "<local>",
                format!(
                    "duplicate corpus identity {}@{}",
                    corpus.library, corpus.version
                ),
            ));
        }
    }
    Ok(())
}

pub fn find_corpus(manifest: Manifest, library: &str, version: &str) -> Result<CorpusRef> {
    manifest
        .corpora
        .into_iter()
        .find(|corpus| corpus.library == library && corpus.version == version)
        .ok_or_else(|| CoreError::CorpusMissing(format!("{library}@{version}")))
}

fn resolve_manifest_artifacts(source: &ManifestSource, manifest: &mut Manifest) -> Result<()> {
    for reference in &mut manifest.corpora {
        reference.url = source.resolve_artifact(&reference.url)?.to_string();
    }
    Ok(())
}

pub async fn fetch_manifest(url: &str) -> Result<Manifest> {
    let source = ManifestSource::parse(url)?;
    let ManifestSource::Https(manifest_url) = &source else {
        return Err(manifest_error(url, "fetch_manifest requires an HTTPS source"));
    };
    let bytes = reqwest::get(manifest_url.as_str())
        .await
        .map_err(|e| CoreError::Manifest { url: url.into(), reason: e.to_string() })?
        .error_for_status()
        .map_err(|e| CoreError::Manifest { url: url.into(), reason: e.to_string() })?
        .bytes()
        .await
        .map_err(|e| CoreError::Manifest { url: url.into(), reason: e.to_string() })?;
    let mut manifest = parse_manifest(&bytes).map_err(|e| CoreError::Manifest {
        url: url.into(),
        reason: e.to_string(),
    })?;
    resolve_manifest_artifacts(&source, &mut manifest)?;
    Ok(manifest)
}

/// Read Parquet rows and write them into a new LanceDB table with its FTS
/// indices. The directory is created fresh; callers rename it into place.
pub async fn build_table_from_parquet(
    db_dir: &std::path::Path,
    parquet: &std::path::Path,
) -> Result<()> {
    use crate::schema::{FTS_COLUMNS, fts_index_params};
    use lancedb::arrow::arrow_array::RecordBatchReader;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    std::fs::create_dir_all(db_dir)?;
    let file = std::fs::File::open(parquet)?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)
        .map_err(|e| CoreError::Io(std::io::Error::other(e)))?
        .build()
        .map_err(|e| CoreError::Io(std::io::Error::other(e)))?;

    let db = lancedb::connect(db_dir.to_str().unwrap()).execute().await?;
    // ParquetRecordBatchReader is already a RecordBatchReader, so batches
    // stream into the table rather than being collected first.
    let reader: Box<dyn RecordBatchReader + Send> = Box::new(reader);
    let table = db.create_table("snippets", reader).execute().await?;
    for column in FTS_COLUMNS {
        table
            .create_index(&[column], lancedb::index::Index::FTS(fts_index_params()))
            .execute()
            .await?;
    }
    Ok(())
}

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

    let fetch_failed = |e: reqwest::Error| CoreError::Download {
        url: reference.url.clone(),
        reason: e.to_string(),
    };
    let bytes = reqwest::get(&reference.url)
        .await
        .map_err(fetch_failed)?
        .error_for_status()
        .map_err(fetch_failed)?
        .bytes()
        .await
        .map_err(fetch_failed)?;
    verify_sha256(&bytes, &reference.sha256)?;

    // Staged inside the cache root so the final rename stays on one
    // filesystem; a rename across mount points fails.
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

    #[test]
    fn a_relative_artifact_resolves_beside_a_local_manifest() {
        let source = ManifestSource::parse("/tmp/corpora/manifest.json").unwrap();
        let resolved = source.resolve_artifact("numpy-2.3.0.tar.zst").unwrap();
        assert_eq!(
            resolved,
            ArtifactSource::File("/tmp/corpora/numpy-2.3.0.tar.zst".into())
        );
    }

    #[test]
    fn a_relative_artifact_resolves_against_an_https_manifest() {
        let source = ManifestSource::parse("https://host.example/releases/manifest.json").unwrap();
        let resolved = source.resolve_artifact("numpy-2.3.0.tar.zst").unwrap();
        assert_eq!(
            resolved.to_string(),
            "https://host.example/releases/numpy-2.3.0.tar.zst"
        );
    }

    #[test]
    fn a_manifest_artifact_is_normalized_before_installation() {
        let source = ManifestSource::parse("https://host.example/releases/manifest.json").unwrap();
        let mut manifest = parse_manifest(
            br#"{"corpora":[{"library":"numpy","version":"2.3.0","url":"numpy-2.3.0.tar.zst","sha256":"a","bytes":1,"embedding_model":"nomic","embedding_dim":768,"snippet_count":1}]}"#,
        )
        .unwrap();

        resolve_manifest_artifacts(&source, &mut manifest).unwrap();

        assert_eq!(
            manifest.corpora[0].url,
            "https://host.example/releases/numpy-2.3.0.tar.zst"
        );
    }

    #[test]
    fn a_manifest_with_the_same_library_and_version_twice_is_rejected() {
        let text = br#"{"corpora":[
          {"library":"numpy","version":"2.2","url":"a.tar.zst","sha256":"a","bytes":1,"embedding_model":"nomic","embedding_dim":768,"snippet_count":1},
          {"library":"numpy","version":"2.2","url":"b.tar.zst","sha256":"b","bytes":1,"embedding_model":"nomic","embedding_dim":768,"snippet_count":1}
        ]}"#;
        assert!(parse_manifest(text).is_err());
    }

    #[test]
    fn a_manifest_with_multiple_versions_of_one_library_is_valid() {
        let text = br#"{"corpora":[
          {"library":"numpy","version":"2.2","url":"a.tar.zst","sha256":"a","bytes":1,"embedding_model":"nomic","embedding_dim":768,"snippet_count":1},
          {"library":"numpy","version":"2.3","url":"b.tar.zst","sha256":"b","bytes":1,"embedding_model":"nomic","embedding_dim":768,"snippet_count":1}
        ]}"#;
        assert!(parse_manifest(text).is_ok());
    }

    #[test]
    fn find_corpus_matches_both_library_and_version() {
        let manifest = parse_manifest(SAMPLE.as_bytes()).unwrap();
        let corpus = find_corpus(manifest, "lancedb", "0.37.1").unwrap();
        assert_eq!(corpus.library, "lancedb");
        assert_eq!(corpus.version, "0.37.1");
    }

    #[test]
    fn a_non_https_remote_manifest_is_rejected() {
        assert!(ManifestSource::parse("ftp://host.example/manifest.json").is_err());
    }

    const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

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
}
