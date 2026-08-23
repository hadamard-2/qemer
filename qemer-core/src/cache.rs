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
        serde_json::from_slice(&bytes).map_err(|e| CoreError::Io(std::io::Error::other(e)))
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
