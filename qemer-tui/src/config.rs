//! Configuration, loaded once at startup.
//!
//! The config type lives in the binary on purpose: a shared `Config` in
//! `qemer-core` would give core a field named for generation, and a future
//! consumer that links core alone would inherit a field it must ignore.
//! Each library crate is handed its own parameters and learns nothing about
//! the other's endpoint.
//!
//! Parsing happens in two stages — a `RawConfig` that serde fills, then a
//! validated `Config` — so that a missing required key produces our message
//! rather than serde's. That message is the entire experience of a first
//! run, which is worth forty lines of plumbing.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("no config file at {path}. Create it and set at least the required keys.")]
    Missing { path: String },
    #[error("config file {path} could not be read: {reason}")]
    Unreadable { path: String, reason: String },
    #[error("config file {path} is not valid TOML: {reason}")]
    Malformed { path: String, reason: String },
    #[error("{key} is required and has no default: {hint}. Set it in {path}.")]
    MissingRequired {
        key: String,
        hint: String,
        path: String,
    },
    #[error("no home directory available, so the config file could not be located")]
    NoHome,
}

fn default_embedding_url() -> String {
    "http://localhost:8080".to_string()
}
fn default_embedding_model() -> String {
    "nomic-embed-text-v1.5".to_string()
}
fn default_embedding_dim() -> usize {
    768
}
fn default_completion_url() -> String {
    "http://localhost:8081".to_string()
}
fn default_completion_model() -> String {
    "qwen3.5-0.8b".to_string()
}
fn default_k() -> usize {
    5
}

/// Exactly what the file contains, with the required values still optional so
/// that their absence is ours to report.
#[derive(Debug, Default, serde::Deserialize)]
struct RawConfig {
    manifest_url: Option<String>,
    #[serde(default)]
    embedding: RawEmbedding,
    #[serde(default)]
    completion: RawCompletion,
    #[serde(default)]
    retrieval: Retrieval,
}

#[derive(Debug, serde::Deserialize)]
struct RawEmbedding {
    #[serde(default = "default_embedding_url")]
    base_url: String,
    #[serde(default = "default_embedding_model")]
    model: String,
    #[serde(default = "default_embedding_dim")]
    dim: usize,
}

impl Default for RawEmbedding {
    fn default() -> Self {
        Self {
            base_url: default_embedding_url(),
            model: default_embedding_model(),
            dim: default_embedding_dim(),
        }
    }
}

/// Only the two required values are optional here. `base_url` and `model`
/// carry real defaults, and `Default` must reproduce them: a missing
/// `[completion]` section goes through `Default`, not through serde's field
/// defaults, so deriving it would silently yield empty strings.
#[derive(Debug, serde::Deserialize)]
struct RawCompletion {
    #[serde(default = "default_completion_url")]
    base_url: String,
    #[serde(default = "default_completion_model")]
    model: String,
    context_tokens: Option<usize>,
    max_completion_tokens: Option<usize>,
}

impl Default for RawCompletion {
    fn default() -> Self {
        Self {
            base_url: default_completion_url(),
            model: default_completion_model(),
            context_tokens: None,
            max_completion_tokens: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub manifest_url: String,
    pub embedding: Embedding,
    pub completion: Completion,
    pub retrieval: Retrieval,
}

#[derive(Debug, Clone)]
pub struct Embedding {
    pub base_url: String,
    pub model: String,
    pub dim: usize,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub base_url: String,
    pub model: String,
    pub context_tokens: usize,
    pub max_completion_tokens: usize,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct Retrieval {
    #[serde(default = "default_k")]
    pub k: usize,
}

impl Default for Retrieval {
    fn default() -> Self {
        Self { k: default_k() }
    }
}

/// Parse and validate. Pure, so every message above is testable without a
/// filesystem.
pub fn parse(text: &str, path: &str) -> Result<Config, ConfigError> {
    let raw: RawConfig = toml::from_str(text).map_err(|e| ConfigError::Malformed {
        path: path.to_string(),
        reason: e.to_string(),
    })?;

    let required = |value: Option<usize>, key: &str, hint: &str| -> Result<usize, ConfigError> {
        value.ok_or_else(|| ConfigError::MissingRequired {
            key: key.to_string(),
            hint: hint.to_string(),
            path: path.to_string(),
        })
    };

    // No value is proposed for either of these. The context length is a
    // property of the running model, and suggesting a number here is how a
    // placeholder becomes a fact by repetition.
    let context_tokens = required(
        raw.completion.context_tokens,
        "context_tokens",
        "read it from the llama-server startup log, which prints the context \
         size when the model loads",
    )?;
    let max_completion_tokens = required(
        raw.completion.max_completion_tokens,
        "max_completion_tokens",
        "choose how many tokens to leave for the answer; the rest of the \
         context is available to the prompt",
    )?;
    let manifest_url = raw.manifest_url.ok_or_else(|| ConfigError::MissingRequired {
        key: "manifest_url".to_string(),
        hint: "the URL of the corpus manifest to install from".to_string(),
        path: path.to_string(),
    })?;

    Ok(Config {
        manifest_url,
        embedding: Embedding {
            base_url: raw.embedding.base_url,
            model: raw.embedding.model,
            dim: raw.embedding.dim,
        },
        completion: Completion {
            base_url: raw.completion.base_url,
            model: raw.completion.model,
            context_tokens,
            max_completion_tokens,
        },
        retrieval: raw.retrieval,
    })
}

/// Where the config file lives. Takes the override as an argument rather than
/// reading the environment, so tests need not mutate process-global state —
/// which `std::env::set_var` makes unsafe in this edition anyway.
pub fn resolve_path(override_var: Option<String>) -> Result<PathBuf, ConfigError> {
    if let Some(path) = override_var {
        return Ok(PathBuf::from(path));
    }
    let dirs = directories::ProjectDirs::from("", "", "qemer").ok_or(ConfigError::NoHome)?;
    Ok(dirs.config_dir().join("config.toml"))
}

pub fn load() -> Result<Config, ConfigError> {
    let path = resolve_path(std::env::var("QEMER_CONFIG").ok())?;
    let shown = path.display().to_string();
    if !path.exists() {
        return Err(ConfigError::Missing { path: shown });
    }
    let text = std::fs::read_to_string(&path).map_err(|e| ConfigError::Unreadable {
        path: shown.clone(),
        reason: e.to_string(),
    })?;
    parse(&text, &shown)
}

impl Config {
    /// Hand `qemer-core` its own parameters. It never learns the completion
    /// endpoint exists.
    pub fn embed_client(&self) -> qemer_core::embed::EmbedClient {
        qemer_core::embed::EmbedClient {
            base_url: self.embedding.base_url.clone(),
            model: self.embedding.model.clone(),
            dim: self.embedding.dim,
        }
    }

    /// Hand `qemer-answer` its own parameters. It never learns the embedding
    /// endpoint exists.
    pub fn generator(&self) -> qemer_answer::Generator {
        qemer_answer::Generator {
            base_url: self.completion.base_url.clone(),
            model: self.completion.model.clone(),
            context_tokens: self.completion.context_tokens,
            max_completion_tokens: self.completion.max_completion_tokens,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every required key present. Individual tests remove one.
    fn complete_toml() -> String {
        r#"
manifest_url = "https://example/manifest.json"

[completion]
context_tokens = 4096
max_completion_tokens = 512
"#
        .to_string()
    }

    #[test]
    fn a_complete_config_parses() {
        let config = parse(&complete_toml(), "/tmp/config.toml").unwrap();
        assert_eq!(config.manifest_url, "https://example/manifest.json");
        assert_eq!(config.completion.context_tokens, 4096);
        assert_eq!(config.completion.max_completion_tokens, 512);
    }

    #[test]
    fn absent_optional_sections_fall_back_to_defaults() {
        let config = parse(&complete_toml(), "/tmp/config.toml").unwrap();
        assert_eq!(config.embedding.model, "nomic-embed-text-v1.5");
        assert_eq!(config.embedding.dim, 768);
        assert_eq!(config.retrieval.k, 5);
        assert!(!config.embedding.base_url.is_empty());
        assert!(!config.completion.base_url.is_empty());
    }

    #[test]
    fn overrides_win_over_defaults() {
        let text = format!("{}\n[retrieval]\nk = 9\n", complete_toml());
        let config = parse(&text, "/tmp/config.toml").unwrap();
        assert_eq!(config.retrieval.k, 9);
    }

    /// The message a first-time user sees. It must name the key, and it must
    /// say where to find the value rather than proposing one.
    #[test]
    fn a_missing_context_length_names_the_key_and_says_where_to_read_it() {
        let text = complete_toml().replace("context_tokens = 4096\n", "");
        let error = parse(&text, "/home/u/.config/qemer/config.toml").unwrap_err();
        let message = error.to_string();
        assert!(message.contains("context_tokens"), "must name the key: {message}");
        assert!(
            message.contains("llama-server"),
            "must say where to read the value: {message}"
        );
        assert!(
            message.contains("/home/u/.config/qemer/config.toml"),
            "must say which file to edit: {message}"
        );
    }

    #[test]
    fn a_missing_completion_cap_names_that_key() {
        let text = complete_toml().replace("max_completion_tokens = 512\n", "");
        let message = parse(&text, "/tmp/config.toml").unwrap_err().to_string();
        assert!(message.contains("max_completion_tokens"), "{message}");
    }

    #[test]
    fn a_missing_manifest_url_names_that_key() {
        let text = complete_toml().replace(
            "manifest_url = \"https://example/manifest.json\"\n",
            "",
        );
        let message = parse(&text, "/tmp/config.toml").unwrap_err().to_string();
        assert!(message.contains("manifest_url"), "{message}");
    }

    /// No number is proposed for either required value, anywhere.
    #[test]
    fn no_error_message_suggests_a_context_length() {
        let text = complete_toml().replace("context_tokens = 4096\n", "");
        let message = parse(&text, "/tmp/config.toml").unwrap_err().to_string();
        for invented in ["2048", "4096", "8192", "16384", "32768", "131072"] {
            assert!(
                !message.contains(invented),
                "message proposes {invented}, which nothing measured: {message}"
            );
        }
    }

    #[test]
    fn malformed_toml_names_the_file() {
        let message = parse("this is not = = toml", "/tmp/config.toml")
            .unwrap_err()
            .to_string();
        assert!(message.contains("/tmp/config.toml"), "{message}");
    }

    #[test]
    fn the_env_override_wins_over_the_xdg_path() {
        let path = resolve_path(Some("/somewhere/else.toml".into())).unwrap();
        assert_eq!(path, std::path::Path::new("/somewhere/else.toml"));
    }

    #[test]
    fn without_an_override_the_path_ends_in_the_expected_file() {
        let path = resolve_path(None).unwrap();
        assert!(
            path.ends_with("qemer/config.toml"),
            "unexpected default path: {}",
            path.display()
        );
    }

    /// The binary hands each crate its own parameters; neither crate learns
    /// about the other's endpoint.
    #[test]
    fn the_clients_receive_their_own_endpoints() {
        let text = format!(
            "{}\n[embedding]\nbase_url = \"http://e:1\"\n",
            complete_toml().replace(
                "[completion]",
                "[completion]\nbase_url = \"http://c:2\""
            )
        );
        let config = parse(&text, "/tmp/config.toml").unwrap();
        assert_eq!(config.embed_client().base_url, "http://e:1");
        assert_eq!(config.generator().base_url, "http://c:2");
    }

    #[test]
    fn the_generator_receives_the_configured_budget() {
        let config = parse(&complete_toml(), "/tmp/config.toml").unwrap();
        let generator = config.generator();
        assert_eq!(generator.context_tokens, 4096);
        assert_eq!(generator.max_completion_tokens, 512);
    }
}
