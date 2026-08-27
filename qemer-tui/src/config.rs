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

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use tempfile::NamedTempFile;

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
    #[error("{key} must be greater than zero")]
    Zero { key: String },
    #[error("max_completion_tokens must not exceed context_tokens")]
    CompletionLimitExceedsContext,
    #[error("config file {path} could not be written: {reason}")]
    Unwritable { path: String, reason: String },
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct Config {
    pub embedding: Embedding,
    pub completion: Completion,
    pub retrieval: Retrieval,
}

/// Values collected by the configuration wizard before they are validated.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConfigDraft {
    pub embedding: Embedding,
    pub completion: Completion,
    pub retrieval: Retrieval,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Embedding {
    pub base_url: String,
    pub model: String,
    pub dim: usize,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Completion {
    pub base_url: String,
    pub model: String,
    pub context_tokens: usize,
    pub max_completion_tokens: usize,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
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
    validate_draft(ConfigDraft {
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

/// The built-in settings used for non-model-specific fields by a first run.
/// The token limits intentionally remain zero until the user supplies them.
pub fn default_draft() -> ConfigDraft {
    ConfigDraft {
        embedding: Embedding {
            base_url: default_embedding_url(),
            model: default_embedding_model(),
            dim: default_embedding_dim(),
        },
        completion: Completion {
            base_url: default_completion_url(),
            model: default_completion_model(),
            context_tokens: 0,
            max_completion_tokens: 0,
        },
        retrieval: Retrieval::default(),
    }
}

impl From<Config> for ConfigDraft {
    fn from(config: Config) -> Self {
        Self {
            embedding: config.embedding,
            completion: config.completion,
            retrieval: config.retrieval,
        }
    }
}

/// Reject configurations that would make a request nonsensical before the
/// values are handed to the retrieval or generation crates.
pub fn validate_draft(draft: ConfigDraft) -> Result<Config, ConfigError> {
    let nonzero = |value: usize, key: &str| {
        (value != 0).then_some(()).ok_or_else(|| ConfigError::Zero {
            key: key.to_string(),
        })
    };
    nonzero(draft.embedding.dim, "embedding.dim")?;
    nonzero(draft.completion.context_tokens, "completion.context_tokens")?;
    nonzero(
        draft.completion.max_completion_tokens,
        "completion.max_completion_tokens",
    )?;
    nonzero(draft.retrieval.k, "retrieval.k")?;
    if draft.completion.max_completion_tokens > draft.completion.context_tokens {
        return Err(ConfigError::CompletionLimitExceedsContext);
    }

    Ok(Config {
        embedding: draft.embedding,
        completion: draft.completion,
        retrieval: draft.retrieval,
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
    load_path(&path)
}

/// Load a known path so the wizard can preserve the error users would see on
/// normal startup instead of overwriting an unreadable or malformed file.
pub fn load_path(path: &Path) -> Result<Config, ConfigError> {
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

/// Atomically replace a configuration file only after its complete TOML has
/// been written to a sibling temporary file.
pub fn write(path: &Path, config: &Config) -> Result<(), ConfigError> {
    let shown = path.display().to_string();
    let parent = write_parent(path);
    std::fs::create_dir_all(parent).map_err(|error| ConfigError::Unwritable {
        path: shown.clone(),
        reason: error.to_string(),
    })?;

    let mut temporary = NamedTempFile::new_in(parent).map_err(|error| ConfigError::Unwritable {
        path: shown.clone(),
        reason: error.to_string(),
    })?;
    let toml = toml::to_string(config).map_err(|error| ConfigError::Unwritable {
        path: shown.clone(),
        reason: error.to_string(),
    })?;
    temporary
        .write_all(toml.as_bytes())
        .map_err(|error| ConfigError::Unwritable {
            path: shown.clone(),
            reason: error.to_string(),
        })?;
    temporary
        .persist(path)
        .map_err(|error| ConfigError::Unwritable {
            path: shown,
            reason: error.error.to_string(),
        })?;
    Ok(())
}

fn write_parent(path: &Path) -> &Path {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
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

    fn valid_draft() -> ConfigDraft {
        ConfigDraft {
            embedding: Embedding {
                base_url: "http://embed.example.test:8080".into(),
                model: "embed-model".into(),
                dim: 768,
            },
            completion: Completion {
                base_url: "http://complete.example.test:8081".into(),
                model: "completion-model".into(),
                context_tokens: 4096,
                max_completion_tokens: 512,
            },
            retrieval: Retrieval { k: 5 },
        }
    }

    /// Every required key present. Individual tests remove one.
    fn complete_toml() -> String {
        r#"
[completion]
context_tokens = 4096
max_completion_tokens = 512
"#
        .to_string()
    }

    #[test]
    fn a_complete_config_parses() {
        let config = parse(&complete_toml(), "/tmp/config.toml").unwrap();
        assert_eq!(config.completion.context_tokens, 4096);
        assert_eq!(config.completion.max_completion_tokens, 512);
    }

    #[test]
    fn a_valid_draft_produces_config() {
        let config = validate_draft(valid_draft()).unwrap();
        assert_eq!(config.embedding.model, "embed-model");
        assert_eq!(config.completion.context_tokens, 4096);
    }

    #[test]
    fn a_zero_embedding_dimension_is_rejected() {
        let mut draft = valid_draft();
        draft.embedding.dim = 0;
        assert!(validate_draft(draft).is_err());
    }

    #[test]
    fn a_zero_context_length_is_rejected() {
        let mut draft = valid_draft();
        draft.completion.context_tokens = 0;
        assert!(validate_draft(draft).is_err());
    }

    #[test]
    fn a_zero_completion_limit_is_rejected() {
        let mut draft = valid_draft();
        draft.completion.max_completion_tokens = 0;
        assert!(validate_draft(draft).is_err());
    }

    #[test]
    fn a_zero_retrieval_count_is_rejected() {
        let mut draft = valid_draft();
        draft.retrieval.k = 0;
        assert!(validate_draft(draft).is_err());
    }

    #[test]
    fn a_completion_limit_larger_than_context_is_rejected() {
        let mut draft = valid_draft();
        draft.completion.max_completion_tokens = draft.completion.context_tokens + 1;
        assert!(validate_draft(draft).is_err());
    }

    #[test]
    fn write_replaces_the_config_with_parseable_toml() {
        let root = std::env::temp_dir().join(format!(
            "qemer-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let path = root.join("qemer/config.toml");
        let config = validate_draft(valid_draft()).unwrap();

        write(&path, &config).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let reparsed = parse(&text, &path.display().to_string()).unwrap();
        assert_eq!(reparsed.embedding.base_url, config.embedding.base_url);
        assert_eq!(reparsed.embedding.model, config.embedding.model);
        assert_eq!(reparsed.embedding.dim, config.embedding.dim);
        assert_eq!(reparsed.completion.base_url, config.completion.base_url);
        assert_eq!(reparsed.completion.model, config.completion.model);
        assert_eq!(
            reparsed.completion.context_tokens,
            config.completion.context_tokens
        );
        assert_eq!(
            reparsed.completion.max_completion_tokens,
            config.completion.max_completion_tokens
        );
        assert_eq!(reparsed.retrieval.k, config.retrieval.k);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_filename_only_config_path_writes_and_reads() {
        struct RestoreWorkingDirectory(std::path::PathBuf);

        impl Drop for RestoreWorkingDirectory {
            fn drop(&mut self) {
                std::env::set_current_dir(&self.0).unwrap();
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let restore = RestoreWorkingDirectory(std::env::current_dir().unwrap());
        std::env::set_current_dir(directory.path()).unwrap();

        let config = validate_draft(valid_draft()).unwrap();
        let path = std::path::Path::new("config.toml");
        write(path, &config).unwrap();
        let loaded = load_path(path).unwrap();

        assert_eq!(loaded.embedding.base_url, config.embedding.base_url);
        assert_eq!(loaded.completion.model, config.completion.model);
        assert_eq!(
            loaded.completion.context_tokens,
            config.completion.context_tokens
        );
        drop(restore);
    }

    #[test]
    fn a_filename_only_path_uses_the_current_directory_for_atomic_write() {
        assert_eq!(
            write_parent(std::path::Path::new("config.toml")),
            std::path::Path::new(".")
        );
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
        assert!(
            message.contains("context_tokens"),
            "must name the key: {message}"
        );
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
            complete_toml().replace("[completion]", "[completion]\nbase_url = \"http://c:2\"")
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
