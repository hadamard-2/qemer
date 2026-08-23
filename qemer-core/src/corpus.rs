//! Discovery, download, and verification of prebuilt corpora.
//!
//! Corpora are built and published by `qemer-ingest`, a separate repository.
//! The contract between the two is this manifest plus the tarball layout;
//! nothing here should assume anything else about how ingestion works.

use crate::Result;

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

pub async fn fetch_manifest(_url: &str) -> Result<Manifest> {
    todo!("GET the manifest and deserialize")
}

pub async fn install(_reference: &CorpusRef) -> Result<Corpus> {
    todo!("download, verify sha256, unpack into the local corpus cache")
}
