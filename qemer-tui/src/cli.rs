//! The non-interactive command line: discover, install, and list corpora.

use color_eyre::Result;
use qemer_core::{Cache, corpus};

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Create or edit Qemer's configuration interactively.
    Config,
    /// Install a corpus, given as `library@version`.
    Install {
        /// For example `lancedb@0.37.1`. The version is required.
        target: String,
        /// A local path or HTTPS URL for the corpus manifest.
        #[arg(long)]
        manifest: String,
    },
    /// List the corpora available from a manifest.
    Available {
        /// A local path or HTTPS URL for the corpus manifest.
        #[arg(long)]
        manifest: String,
    },
    /// List the corpora already installed.
    List,
}

/// Split `library@version`. The version is required: defaulting it would
/// answer a question `docs/decisions.md` records as still open.
pub fn parse_target(target: &str) -> Result<(String, String), String> {
    let malformed =
        || format!("expected `library@version`, for example `lancedb@0.37.1`, but got `{target}`");
    let mut parts = target.split('@');
    let (Some(library), Some(version), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(malformed());
    };
    if library.is_empty() || version.is_empty() {
        return Err(malformed());
    }
    Ok((library.to_string(), version.to_string()))
}

pub async fn available(manifest: &str) -> Result<()> {
    let mut references = corpus::load_manifest(manifest).await?.corpora;
    references.sort_by(|left, right| {
        (left.library.as_str(), left.version.as_str())
            .cmp(&(right.library.as_str(), right.version.as_str()))
    });
    for reference in references {
        println!(
            "{}@{} · {} snippets · {} bytes",
            reference.library, reference.version, reference.snippet_count, reference.bytes
        );
    }
    Ok(())
}

pub async fn install(target: &str, manifest: &str) -> Result<()> {
    let (library, version) = parse_target(target).map_err(|e| color_eyre::eyre::eyre!(e))?;
    let manifest = corpus::load_manifest(manifest).await?;
    let reference = corpus::find_corpus(manifest, &library, &version)?;

    let cache = Cache::new(Cache::default_root()?);
    println!(
        "installing {}@{} ({} snippets) …",
        reference.library, reference.version, reference.snippet_count
    );
    let installed = corpus::install(&cache, &reference).await?;
    println!("installed to {}", installed.path.display());
    Ok(())
}

pub fn list() -> Result<()> {
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
        assert_eq!(
            parse_target("numpy@2.3.0").unwrap(),
            ("numpy".into(), "2.3.0".into())
        );
    }

    /// The version is required because defaulting it to "newest" would
    /// silently answer the version-selection question docs/decisions.md
    /// records as open.
    #[test]
    fn a_target_without_a_version_is_rejected() {
        assert!(parse_target("numpy").is_err());
    }

    #[test]
    fn available_and_install_require_a_manifest_option() {
        use clap::CommandFactory;

        let command = crate::Args::command();
        for name in ["available", "install"] {
            let subcommand = command
                .get_subcommands()
                .find(|command| command.get_name() == name)
                .unwrap();
            assert!(
                subcommand
                    .get_arguments()
                    .any(|argument| argument.get_long() == Some("manifest"))
            );
        }
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
