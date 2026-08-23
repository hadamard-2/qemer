//! Discovery, download, and verification of prebuilt corpora.
//!
//! Corpora are built and published by `qemer-ingest`, a separate repository.
//! The contract between the two is this manifest plus the tarball layout;
//! nothing here should assume anything else about how ingestion works.

use crate::{CoreError, Result};

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

pub async fn install(_reference: &CorpusRef) -> Result<Corpus> {
    todo!("download, verify sha256, unpack into the local corpus cache")
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
