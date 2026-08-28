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

fn encode_identity_component(component: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(component.len() * 2);
    for byte in component.bytes() {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

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
        let library = encode_identity_component(library);
        let version = encode_identity_component(version);
        self.root.join(format!("v1-{library}-{version}"))
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
            if !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("v1-"))
            {
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
            std::path::Path::new("/tmp/root/v1-6c616e63656462-302e33372e31")
        );
    }

    #[test]
    fn untrusted_identity_values_cannot_escape_the_cache_root() {
        let cache = Cache::new("/tmp/qemer-cache".into());

        let dir = cache.dir_for("../../../outside", "../../other");

        assert_eq!(dir.parent(), Some(cache.root.as_path()));
        assert_eq!(
            dir.file_name().unwrap(),
            "v1-2e2e2f2e2e2f2e2e2f6f757473696465-2e2e2f2e2e2f6f74686572"
        );
    }

    #[test]
    fn component_encoding_avoids_delimiter_and_case_folded_collisions() {
        let cache = Cache::new("/tmp/qemer-cache".into());

        assert_ne!(cache.dir_for("a-b", "c"), cache.dir_for("a", "b-c"));
        assert_ne!(cache.dir_for("NumPy", "2.3"), cache.dir_for("numpy", "2.3"));
    }

    #[test]
    fn component_encoding_is_reversible_for_utf8() {
        fn decode(encoded: &str) -> String {
            let bytes = encoded
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| {
                    let pair = std::str::from_utf8(pair).unwrap();
                    u8::from_str_radix(pair, 16).unwrap()
                })
                .collect::<Vec<_>>();
            String::from_utf8(bytes).unwrap()
        }

        let cache = Cache::new("/tmp/qemer-cache".into());
        let dir = cache.dir_for("NumPy/数组", "2.3-β");
        let name = dir.file_name().unwrap().to_str().unwrap();
        let (library, version) = name.strip_prefix("v1-").unwrap().split_once('-').unwrap();

        assert_eq!(decode(library), "NumPy/数组");
        assert_eq!(decode(version), "2.3-β");
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

    #[test]
    fn installed_ignores_legacy_layout_directories() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = Cache::new(tmp.path().to_path_buf());

        let legacy_dir = tmp.path().join("lancedb-0.37.1");
        std::fs::create_dir_all(&legacy_dir).unwrap();
        cache.write_meta(&legacy_dir, &a_ref()).unwrap();

        let v1_dir = cache.dir_for("lancedb", "0.37.1");
        std::fs::create_dir_all(&v1_dir).unwrap();
        cache.write_meta(&v1_dir, &a_ref()).unwrap();

        let found = cache.installed().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, v1_dir);
    }
}
