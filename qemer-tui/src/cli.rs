//! The non-interactive command line: install a corpus, list what is installed.
//!
//! Deliberately plain. Browsing the manifest, choosing versions, and showing
//! a download in progress are open questions in `docs/decisions.md`, and
//! building even a small version of them here would bias the design later.

use crate::config::Config;
use color_eyre::Result;
use qemer_core::{Cache, corpus};

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Install a corpus, given as `library@version`.
    Install {
        /// For example `lancedb@0.37.1`. The version is required.
        target: String,
    },
    /// List the corpora already installed.
    List,
}

/// Split `library@version`. The version is required: defaulting it would
/// answer a question `docs/decisions.md` records as still open.
pub fn parse_target(target: &str) -> Result<(String, String), String> {
    let malformed = || {
        format!(
            "expected `library@version`, for example `lancedb@0.37.1`, but got `{target}`"
        )
    };
    let mut parts = target.split('@');
    let (Some(library), Some(version), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(malformed());
    };
    if library.is_empty() || version.is_empty() {
        return Err(malformed());
    }
    Ok((library.to_string(), version.to_string()))
}

pub async fn install(config: &Config, target: &str) -> Result<()> {
    let (library, version) = parse_target(target).map_err(|e| color_eyre::eyre::eyre!(e))?;
    let manifest = corpus::fetch_manifest(&config.manifest_url).await?;
    let reference = manifest
        .corpora
        .into_iter()
        .find(|c| c.library == library && c.version == version)
        .ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "the manifest at {} lists no corpus `{library}@{version}`",
                config.manifest_url
            )
        })?;

    let cache = Cache::new(Cache::default_root()?);
    println!(
        "installing {library}@{version} ({} snippets, {} bytes) …",
        reference.snippet_count, reference.bytes
    );
    let installed = corpus::install(&cache, &reference).await?;
    println!("installed to {}", installed.path.display());
    Ok(())
}

pub fn list(_config: &Config) -> Result<()> {
    let cache = Cache::new(Cache::default_root()?);
    let installed = cache.installed()?;
    if installed.is_empty() {
        println!("no corpora installed");
        return Ok(());
    }
    for corpus in installed {
        println!(
            "{} {} · {} snippets",
            corpus.reference.library, corpus.reference.version, corpus.reference.snippet_count
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_splits_into_library_and_version() {
        let (library, version) = parse_target("lancedb@0.37.1").unwrap();
        assert_eq!(library, "lancedb");
        assert_eq!(version, "0.37.1");
    }

    /// The version is required because defaulting it to "newest" would
    /// silently answer the version-selection question docs/decisions.md
    /// records as open.
    #[test]
    fn a_target_without_a_version_is_rejected() {
        let error = parse_target("lancedb").unwrap_err();
        assert!(error.contains("lancedb@"), "must show the expected form: {error}");
    }

    #[test]
    fn an_empty_library_is_rejected() {
        assert!(parse_target("@0.37.1").is_err());
    }

    #[test]
    fn an_empty_version_is_rejected() {
        assert!(parse_target("lancedb@").is_err());
    }

    /// Versions contain dots but not at signs; splitting on the first at sign
    /// keeps a library name containing one from being silently truncated.
    #[test]
    fn only_the_first_at_sign_separates() {
        assert!(parse_target("lance@db@0.1").is_err());
    }
}
