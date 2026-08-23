# Qemer TUI Query Loop Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `qemer-tui` to the point where a user can pick an installed corpus, ask a question, and watch a grounded answer stream in with its sources beneath it — plus a command-line path to install corpora in the first place.

**Architecture:** State and rendering are separated so that everything worth asserting is a pure function over data: `config.rs` parses and validates, `query.rs` maps crate errors to user-facing text, `app.rs` turns a key into a state transition plus an `Action`, and `view.rs` only draws. The whole query — search *and* generation — is one `async_stream`, so the event loop holds a single `Option<QueryStream>` and aborting is dropping it.

**Tech Stack:** Rust 2024, `ratatui` 0.30.2, `crossterm` 0.29 (via `ratatui::crossterm`), `clap` 4 derive, `toml` 0.9, `async-stream` 0.3, `tokio` 1.53, `futures` 0.3.

**Spec:** [`docs/superpowers/specs/2026-08-24-qemer-tui-query-loop-design.md`](../specs/2026-08-24-qemer-tui-query-loop-design.md) — read it first, along with [`docs/decisions.md`](../../decisions.md), which records what is settled *and what is deliberately still open*.

## Global Constraints

- **No corpus browsing, version selection, or cache eviction.** All three are open questions in `docs/decisions.md`. Installing is `qemer install <library>@<version>` and nothing more. If a task appears to need one of them resolved, stop and ask.
- **The version argument to `qemer install` is required, never defaulted.** Defaulting to "newest available" silently answers the open version-selection question.
- **`context_tokens`, `max_completion_tokens`, and `manifest_url` are required config keys with no defaults.** Do not invent a value for any of them anywhere — not in code, not in a doc comment, not in a test fixture presented as a recommendation. Test fixtures may use arbitrary numbers; nothing user-facing may suggest one.
- **Crossterm types are imported through `ratatui::crossterm`, never from a direct `use crossterm::...`.** The direct dependency exists only to enable the `event-stream` feature. This mirrors the Arrow rule in `docs/decisions.md`: two identically named `KeyEvent` types is a trait-bound failure at the call site.
- **The two endpoints are reported separately and never blur.** A retrieval failure names the embedding endpoint; a generation failure names the completion endpoint. Neither message may mention the other.
- **No terminal harness for logic that never touches a terminal**, per `CLAUDE.md`. `view.rs` may use `TestBackend` sparingly; `app.rs`, `config.rs`, and `query.rs` must be testable without one.
- **Never probe `llama-server` at startup.** Reachability is discovered at the point of use.
- Commits follow Conventional Commits: `<type>: <subject>`, bulleted body when the commit has more than one distinct sub-change.

## Verified facts about the dependencies

Checked against the crates in `~/.cargo/registry` at the versions this workspace resolves, not recalled. Tasks below depend on all of these.

- **`crossterm`'s `EventStream` is gated behind the non-default `event-stream` feature** (`crossterm-0.29.0/src/event.rs:124`). `qemer-tui/Cargo.toml` currently declares `crossterm = "0.29.0"` with no features, so Task 6 does not compile until that changes. Neither `ratatui` nor `ratatui-crossterm` offers a passthrough for it.
- **`ratatui` 0.30.2 re-exports crossterm** as `ratatui::crossterm` (`ratatui-0.30.2/src/lib.rs:483`), selecting the version through its `crossterm_0_29` feature (on by default).
- **`ratatui::init()`, `try_init()`, and `restore()` exist** in `ratatui-0.30.2/src/init.rs`; `DefaultTerminal` is `Terminal<CrosstermBackend<Stdout>>` (`init.rs:213`).
- **`Frame::area()` is the viewport accessor** (`ratatui-core-0.1.2/src/terminal/frame.rs:68`), and `Terminal::draw` takes `FnOnce(&mut Frame)`.
- **`ListState::select_next()` does not clamp at the end of the list** (`ratatui-widgets-0.3.2/src/list/state.rs:177`) — it increments and lets rendering clamp. This plan therefore keeps the selected index as a plain `usize` on `App` and does its own bounds arithmetic, which is what makes it testable.
- `Paragraph::scroll` takes `(vertical, horizontal)` as a plain `(u16, u16)` (`paragraph.rs:127-128`, where `Vertical` and `Horizontal` are `u16` aliases).

## Verified facts about `qemer-core` and `qemer-answer`

- `Cache::installed() -> Result<Vec<Corpus>>` already sorts by library then version, and silently skips directories without readable metadata.
- `search::search(&Corpus, &EmbedClient, &str, k: usize) -> Result<Vec<Snippet>>`.
- `EmbedClient` is a plain struct with public `base_url`, `model`, `dim`.
- `Generator` is a plain struct with public `base_url`, `model`, `context_tokens`, `max_completion_tokens`, and `answer(&self, &str, &[Snippet]) -> impl Stream<Item = Result<AnswerEvent, AnswerError>>`.
- `corpus::fetch_manifest(&str) -> Result<Manifest>` and `corpus::install(&Cache, &CorpusRef) -> Result<Corpus>`.

## Choices this plan makes that you may want to override

- **Config is parsed in two stages: a `RawConfig` that serde fills, then a validated `Config`.** The alternative is `Option<usize>` fields carried through the whole program. Two stages costs about forty lines and buys exact control over the message a missing key produces — which matters because that message is the entire user experience of a first run.
- **`handle_key` mutates `App` and returns an `Action`.** Returning an intent rather than performing I/O is what lets every key rule be tested without a runtime. A pure `fn(App, Key) -> App` would be tidier still but would force the whole state to be cloned per keystroke for no gain.
- **`clap` is used for two subcommands.** Hand-rolled `std::env::args` matching would be about twenty lines with no new dependency. This is the cheapest decision here to reverse.

## File Structure

| File | Responsibility |
| --- | --- |
| `qemer-tui/src/config.rs` | **New.** `Config`, TOML parsing, validation, path resolution, and the constructors that hand each library crate its own parameters. |
| `qemer-tui/src/cli.rs` | **New.** `install` and `list`. Non-interactive, prints to stdout, never touches ratatui. |
| `qemer-tui/src/query.rs` | **New.** `QueryEvent`, `QueryError`, the unified query stream, and the error-to-message mapping. |
| `qemer-tui/src/app.rs` | **New.** `App`, `Screen`, `Status`, `Action`, `handle_key`, and the `select!` event loop. |
| `qemer-tui/src/view.rs` | **New.** Rendering only. Takes `&App`, draws, holds nothing. |
| `qemer-tui/src/main.rs` | **Exists as a stub.** Argument dispatch, panic hook, terminal setup and teardown. |

---

## Task 1: Configuration

The first thing that runs and the first thing that can fail. Everything else takes its parameters from here, so this task establishes the shape the rest of the program consumes.

**Files:**
- Create: `qemer-tui/src/config.rs`
- Modify: `qemer-tui/src/main.rs` (add `mod config;`)
- Modify: `qemer-tui/Cargo.toml`

**Interfaces:**
- Consumes: `qemer_core::EmbedClient`, `qemer_answer::Generator`.
- Produces: `config::Config { manifest_url, embedding, completion, retrieval }`; `config::ConfigError`; `config::parse(text: &str, path: &str) -> Result<Config, ConfigError>`; `config::resolve_path(override_var: Option<String>) -> Result<PathBuf, ConfigError>`; `config::load() -> Result<Config, ConfigError>`; `Config::embed_client(&self) -> EmbedClient`; `Config::generator(&self) -> Generator`.

- [ ] **Step 1: Add the dependencies**

In `qemer-tui/Cargo.toml`, add to `[dependencies]`:

```toml
directories = "6"
serde = { version = "1.0.229", features = ["derive"] }
thiserror = "2.0.20"
toml = "0.9"
```

Versions for `serde` and `thiserror` match `qemer-core/Cargo.toml` so the workspace resolves one copy of each.

- [ ] **Step 2: Write the failing tests**

Create `qemer-tui/src/config.rs` containing only this test module for now.

```rust
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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p qemer-tui config`
Expected: FAIL to compile — `parse`, `resolve_path`, `Config`, and `ConfigError` do not exist.

- [ ] **Step 4: Write the implementation**

Prepend to `qemer-tui/src/config.rs`, above the test module.

```rust
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
```

Note why `RawEmbedding` and `RawCompletion` both implement `Default` by hand rather than deriving it. When a whole section is absent from the file, serde uses the struct's `Default`, not the per-field `#[serde(default = ...)]` attributes — so a derived `Default` would give an empty `base_url` for anyone who omits `[embedding]`, which is the common case. The hand-written impl is what makes `absent_optional_sections_fall_back_to_defaults` pass.

- [ ] **Step 5: Register the module**

Replace `qemer-tui/src/main.rs` with:

```rust
//! Qemer TUI: pick a library, ask a question, read the sources and the answer.

mod config;

use color_eyre::Result;

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    println!("qemer: skeleton — no UI yet");
    Ok(())
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p qemer-tui config`
Expected: PASS, 12 tests.

If `without_an_override_the_path_ends_in_the_expected_file` fails in a sandbox with no home directory, that is the environment rather than the code — report it rather than weakening the assertion.

- [ ] **Step 7: Commit**

```bash
git add qemer-tui/src/config.rs qemer-tui/src/main.rs qemer-tui/Cargo.toml Cargo.lock
git commit -m "feat(tui): load and validate configuration

- Parse in two stages so a missing required key produces our message
  rather than serde's; that message is the whole experience of a first
  run.
- context_tokens, max_completion_tokens and manifest_url are required
  with no defaults, and no error text proposes a value for any of them.
- Config hands each library crate its own parameters, so neither learns
  the other's endpoint."
```

---

## Task 2: The install and list subcommands

The only way to get a corpus onto disk in this slice. Non-interactive by design: it settles none of the deferred questions about how browsing should look.

**Files:**
- Create: `qemer-tui/src/cli.rs`
- Modify: `qemer-tui/src/main.rs`
- Modify: `qemer-tui/Cargo.toml`

**Interfaces:**
- Consumes: `config::Config`, `qemer_core::{Cache, corpus}`.
- Produces: `cli::Command` enum; `cli::parse_target(&str) -> Result<(String, String), String>`; `cli::install(&Config, &str) -> Result<()>`; `cli::list(&Config) -> Result<()>`.

- [ ] **Step 1: Add the dependency**

In `qemer-tui/Cargo.toml`:

```toml
clap = { version = "4.5", features = ["derive"] }
```

- [ ] **Step 2: Write the failing tests**

Create `qemer-tui/src/cli.rs` with only the test module.

```rust
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
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p qemer-tui cli`
Expected: FAIL to compile — `parse_target` does not exist.

- [ ] **Step 4: Write the implementation**

Prepend to `qemer-tui/src/cli.rs`.

```rust
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
```

- [ ] **Step 5: Wire up argument dispatch**

Replace `qemer-tui/src/main.rs`:

```rust
//! Qemer TUI: pick a library, ask a question, read the sources and the answer.

mod cli;
mod config;

use clap::Parser;
use color_eyre::Result;

#[derive(Debug, clap::Parser)]
#[command(name = "qemer", about = "Offline coding help grounded in documentation")]
struct Args {
    #[command(subcommand)]
    command: Option<cli::Command>,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    let args = Args::parse();

    // Configuration is read before anything touches the terminal, so a
    // validation failure lands on an ordinary screen the user can read.
    let config = config::load()?;

    match args.command {
        Some(cli::Command::Install { target }) => cli::install(&config, &target).await,
        Some(cli::Command::List) => cli::list(&config),
        None => {
            println!("qemer: TUI not wired up yet");
            Ok(())
        }
    }
}
```

- [ ] **Step 6: Run the tests and check the binary**

Run: `cargo test -p qemer-tui`
Expected: PASS, 17 tests.

Run: `cargo run -p qemer-tui -- --help`
Expected: help text listing `install` and `list`. It may instead fail with the config error if no config file exists — that is correct behaviour, not a bug, though it does mean `--help` needs a config. Note it and continue; Task 6 revisits ordering.

- [ ] **Step 7: Commit**

```bash
git add qemer-tui/src/cli.rs qemer-tui/src/main.rs qemer-tui/Cargo.toml Cargo.lock
git commit -m "feat(tui): install and list corpora from the command line

- `qemer install library@version` and `qemer list`, deliberately plain:
  browsing and progress display are open questions and building a small
  version of them here would bias the real design.
- The version argument is required rather than defaulting to newest,
  which would silently settle an open question."
```

---

## Task 3: The unified query stream

Where the two library crates are joined, and the only place that knows both endpoints exist. The tested part is the error mapping, because that is the part with rules.

**Files:**
- Create: `qemer-tui/src/query.rs`
- Modify: `qemer-tui/src/main.rs` (add `mod query;`)
- Modify: `qemer-tui/Cargo.toml`

**Interfaces:**
- Consumes: `qemer_core::{Corpus, CoreError, Snippet, embed::EmbedClient, search}`, `qemer_answer::{AnswerError, AnswerEvent, Generator}`.
- Produces: `query::QueryEvent`; `query::QueryError`; `query::describe_retrieval_failure(&CoreError, embedding_url: &str) -> String`; `query::describe_generation_failure(&AnswerError, completion_url: &str) -> String`; `query::run(Corpus, EmbedClient, Generator, String, usize) -> impl Stream<Item = Result<QueryEvent, QueryError>>`.

- [ ] **Step 1: Add the dependency**

In `qemer-tui/Cargo.toml`:

```toml
async-stream = "0.3"
```

- [ ] **Step 2: Write the failing tests**

Create `qemer-tui/src/query.rs` with only the test module.

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const EMBEDDING_URL: &str = "http://localhost:8080";
    const COMPLETION_URL: &str = "http://localhost:8081";

    #[test]
    fn an_unreachable_embedding_server_names_that_endpoint_and_what_to_start() {
        let error = CoreError::Embed("connection refused".into());
        let message = describe_retrieval_failure(&error, EMBEDDING_URL);
        assert!(message.contains(EMBEDDING_URL), "{message}");
        assert!(message.contains("llama-server"), "{message}");
    }

    /// The two endpoints fail independently and are configured separately.
    /// A retrieval failure that mentions the completion endpoint would send
    /// the user to restart the wrong server.
    #[test]
    fn a_retrieval_failure_never_mentions_the_completion_endpoint() {
        let error = CoreError::Embed("connection refused".into());
        let message = describe_retrieval_failure(&error, EMBEDDING_URL);
        assert!(!message.contains(COMPLETION_URL), "{message}");
        assert!(!message.to_lowercase().contains("completion"), "{message}");
    }

    #[test]
    fn a_generation_failure_never_mentions_the_embedding_endpoint() {
        let error = AnswerError::Unreachable(format!("{COMPLETION_URL}: connection refused"));
        let message = describe_generation_failure(&error, COMPLETION_URL);
        assert!(message.contains(COMPLETION_URL), "{message}");
        assert!(!message.contains(EMBEDDING_URL), "{message}");
        assert!(!message.to_lowercase().contains("embedding"), "{message}");
    }

    /// A model mismatch is already precise about what is wrong, and no
    /// server needs restarting. Telling the user to start llama-server would
    /// be actively misleading.
    #[test]
    fn a_model_mismatch_is_passed_through_without_start_advice() {
        let error = CoreError::ModelMismatch {
            corpus: "lancedb-0.37.1".into(),
            corpus_model: "nomic-embed-text-v1.5".into(),
            corpus_dim: 768,
            client_model: "other-model".into(),
            client_dim: 384,
        };
        let message = describe_retrieval_failure(&error, EMBEDDING_URL);
        assert!(message.contains("lancedb-0.37.1"), "{message}");
        assert!(message.contains("other-model"), "{message}");
        assert!(
            !message.contains("llama-server"),
            "restarting a server does not fix a mismatched corpus: {message}"
        );
    }

    #[test]
    fn a_missing_corpus_is_passed_through_without_start_advice() {
        let error = CoreError::CorpusMissing("lancedb".into());
        let message = describe_retrieval_failure(&error, EMBEDDING_URL);
        assert!(message.contains("lancedb"), "{message}");
        assert!(!message.contains("llama-server"), "{message}");
    }

    #[test]
    fn a_generation_error_that_is_not_unreachability_is_passed_through() {
        let error = AnswerError::Generation("HTTP 500".into());
        let message = describe_generation_failure(&error, COMPLETION_URL);
        assert!(message.contains("HTTP 500"), "{message}");
        assert!(!message.contains("llama-server"), "{message}");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p qemer-tui query`
Expected: FAIL to compile — `describe_retrieval_failure` and `describe_generation_failure` do not exist.

- [ ] **Step 4: Write the implementation**

Prepend to `qemer-tui/src/query.rs`.

```rust
//! One query, start to finish, as a single stream.
//!
//! Generation is a stream but search is a single await — an embedding
//! round-trip plus a database query. Awaiting search directly would freeze
//! the interface for its duration, including through a network timeout when
//! the embedding server is down, which is exactly when a user wants out.
//! Wrapping both phases in one stream gives search the same escape hatch:
//! dropping the stream cancels whichever phase is live.
//!
//! This module is also the one place that knows both endpoints exist. The
//! library crates each name only their own, and report failures without
//! advice, precisely so that the caller — which knows which endpoint it
//! wanted — supplies it.

use futures::{Stream, StreamExt};
use qemer_answer::{AnswerError, AnswerEvent, Generator};
use qemer_core::embed::EmbedClient;
use qemer_core::{Corpus, CoreError, Snippet, search};

/// What the interface learns as a query progresses.
#[derive(Debug, Clone)]
pub enum QueryEvent {
    /// Retrieval has started. Emitted first so the interface can say so
    /// before the embedding round-trip returns.
    Searching,
    /// Retrieval finished. Emitted before any token, so the grounding is on
    /// screen while the model is still working — which matters most when the
    /// answer turns out to be wrong.
    Snippets(Vec<Snippet>),
    Token(String),
    Done {
        prompt_tokens: usize,
        completion_tokens: usize,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("{0}")]
    Retrieval(String),
    #[error("{0}")]
    Generation(String),
}

/// Describe a retrieval failure. Names the embedding endpoint and never the
/// completion one.
pub fn describe_retrieval_failure(error: &CoreError, embedding_url: &str) -> String {
    match error {
        // Only unreachability is worth advice. A mismatch or a missing
        // corpus is already precise, and restarting a server fixes neither.
        CoreError::Embed(reason) => format!(
            "Could not reach the embedding server at {embedding_url} ({reason}). \
             Start llama-server with your embedding model on that address, then ask again."
        ),
        other => other.to_string(),
    }
}

/// Describe a generation failure. Names the completion endpoint and never the
/// embedding one.
pub fn describe_generation_failure(error: &AnswerError, completion_url: &str) -> String {
    match error {
        AnswerError::Unreachable(_) => format!(
            "Could not reach the completion server at {completion_url}. \
             Start llama-server with your chat model on that address, then ask again."
        ),
        other => other.to_string(),
    }
}

/// Run one query: retrieve, then generate, as a single cancellable stream.
///
/// Everything is taken by value so the returned stream borrows nothing and
/// the caller may hold it for as long as it likes.
pub fn run(
    corpus: Corpus,
    embed: EmbedClient,
    generator: Generator,
    query: String,
    k: usize,
) -> impl Stream<Item = Result<QueryEvent, QueryError>> {
    async_stream::try_stream! {
        yield QueryEvent::Searching;

        let snippets = search::search(&corpus, &embed, &query, k)
            .await
            .map_err(|e| QueryError::Retrieval(describe_retrieval_failure(&e, &embed.base_url)))?;
        yield QueryEvent::Snippets(snippets.clone());

        let answer = generator.answer(&query, &snippets);
        let mut answer = std::pin::pin!(answer);
        while let Some(event) = answer.next().await {
            let event = event.map_err(|e| {
                QueryError::Generation(describe_generation_failure(&e, &generator.base_url))
            })?;
            match event {
                AnswerEvent::Token(text) => yield QueryEvent::Token(text),
                AnswerEvent::Done { prompt_tokens, completion_tokens } => {
                    yield QueryEvent::Done { prompt_tokens, completion_tokens };
                }
            }
        }
    }
}
```

- [ ] **Step 5: Register the module**

Add `mod query;` beside `mod cli;` and `mod config;` in `qemer-tui/src/main.rs`.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p qemer-tui query`
Expected: PASS, 6 tests.

If the compiler reports that `answer` borrows `generator` for too short a lifetime, keep the two-line `let answer = …; let mut answer = std::pin::pin!(answer);` form rather than inlining it — `pin!` needs the value to live in its own binding.

- [ ] **Step 7: Commit**

```bash
git add qemer-tui/src/query.rs qemer-tui/src/main.rs qemer-tui/Cargo.toml Cargo.lock
git commit -m "feat(tui): run retrieval and generation as one cancellable stream

- Wrapping search and generation together gives search the same
  drop-to-cancel escape hatch generation already has; awaiting search
  directly would freeze the interface through a network timeout.
- Snippets are emitted before the first token so the grounding is
  visible while the model is still working.
- Failures name the endpoint that actually failed and never the other;
  advice is added only for unreachability, since a model mismatch is
  not fixed by restarting anything."
```

---

## Task 4: Application state and key handling

Every rule about what a key does, expressed as a pure transition and tested without a terminal.

**Files:**
- Create: `qemer-tui/src/app.rs`
- Modify: `qemer-tui/src/main.rs` (add `mod app;`)

**Interfaces:**
- Consumes: `config::Config`, `query::{QueryError, QueryEvent}`, `qemer_core::{Corpus, Snippet}`.
- Produces: `app::App`; `app::Screen`; `app::Status`; `app::Action`; `App::new(Vec<Corpus>) -> App`; `App::handle_key(&mut self, KeyEvent) -> Action`; `App::apply(&mut self, Result<QueryEvent, QueryError>)`; `App::selected_corpus(&self) -> Option<&Corpus>`; `App::is_busy(&self) -> bool`.

- [ ] **Step 1: Write the failing tests**

Create `qemer-tui/src/app.rs` with only the test module.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use qemer_core::CorpusRef;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn a_corpus(library: &str) -> Corpus {
        Corpus {
            reference: CorpusRef {
                library: library.into(),
                version: "0.1.0".into(),
                url: "https://example/x.tar.zst".into(),
                sha256: "abc".into(),
                bytes: 1,
                embedding_model: "nomic-embed-text-v1.5".into(),
                embedding_dim: 768,
                snippet_count: 3,
            },
            path: std::path::PathBuf::from("/tmp/x"),
        }
    }

    fn an_app() -> App {
        App::new(vec![a_corpus("alpha"), a_corpus("beta"), a_corpus("gamma")])
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn on_query_screen() -> App {
        let mut app = an_app();
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.screen, Screen::Query));
        app
    }

    #[test]
    fn a_fresh_app_starts_on_the_picker_with_the_first_row_selected() {
        let app = an_app();
        assert!(matches!(app.screen, Screen::Picker));
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn moving_down_and_up_walks_the_list() {
        let mut app = an_app();
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.selected, 1);
        app.handle_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected, 2);
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.selected, 1);
        app.handle_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected, 0);
    }

    /// ratatui's ListState does not clamp, so the bounds are ours to enforce
    /// and therefore ours to test.
    #[test]
    fn the_selection_does_not_run_off_either_end() {
        let mut app = an_app();
        for _ in 0..10 {
            app.handle_key(key(KeyCode::Up));
        }
        assert_eq!(app.selected, 0);
        for _ in 0..10 {
            app.handle_key(key(KeyCode::Down));
        }
        assert_eq!(app.selected, 2, "three corpora means the last index is 2");
    }

    #[test]
    fn an_empty_picker_has_no_selected_corpus_and_enter_does_nothing() {
        let mut app = App::new(vec![]);
        assert!(app.selected_corpus().is_none());
        app.handle_key(key(KeyCode::Enter));
        assert!(matches!(app.screen, Screen::Picker));
    }

    #[test]
    fn quitting_from_the_picker_is_requested_not_performed() {
        let mut app = an_app();
        assert!(matches!(app.handle_key(key(KeyCode::Char('q'))), Action::Quit));
    }

    #[test]
    fn typing_accumulates_into_the_input_line() {
        let mut app = on_query_screen();
        for c in "how?".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        assert_eq!(app.input, "how?");
        app.handle_key(key(KeyCode::Backspace));
        assert_eq!(app.input, "how");
    }

    #[test]
    fn enter_with_text_asks_the_caller_to_start_a_query() {
        let mut app = on_query_screen();
        for c in "search".chars() {
            app.handle_key(key(KeyCode::Char(c)));
        }
        match app.handle_key(key(KeyCode::Enter)) {
            Action::StartQuery(q) => assert_eq!(q, "search"),
            other => panic!("expected StartQuery, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_an_empty_line_does_nothing() {
        let mut app = on_query_screen();
        assert!(matches!(app.handle_key(key(KeyCode::Enter)), Action::None));
    }

    /// Generation blocks input, which is why there is never a second stream
    /// and nothing to interleave.
    #[test]
    fn enter_is_ignored_while_a_query_is_in_flight() {
        let mut app = on_query_screen();
        app.input = "another".into();
        app.status = Status::Streaming;
        assert!(matches!(app.handle_key(key(KeyCode::Enter)), Action::None));
    }

    #[test]
    fn typing_is_ignored_while_a_query_is_in_flight() {
        let mut app = on_query_screen();
        app.status = Status::Searching;
        app.handle_key(key(KeyCode::Char('x')));
        assert_eq!(app.input, "", "input is blocked, not queued");
    }

    #[test]
    fn escape_aborts_while_a_query_is_in_flight() {
        let mut app = on_query_screen();
        app.status = Status::Streaming;
        assert!(matches!(app.handle_key(key(KeyCode::Esc)), Action::Abort));
        assert!(matches!(app.screen, Screen::Query), "abort does not navigate");
    }

    #[test]
    fn escape_returns_to_the_picker_while_idle() {
        let mut app = on_query_screen();
        app.handle_key(key(KeyCode::Esc));
        assert!(matches!(app.screen, Screen::Picker));
    }

    #[test]
    fn ctrl_c_quits_from_anywhere() {
        let mut app = on_query_screen();
        app.status = Status::Streaming;
        let event = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(matches!(app.handle_key(event), Action::Quit));
    }

    #[test]
    fn starting_a_query_clears_the_previous_answer_and_sources() {
        let mut app = on_query_screen();
        app.answer = "old answer".into();
        app.sources = vec![];
        app.error = Some("old error".into());
        app.input = "new".into();
        app.handle_key(key(KeyCode::Enter));
        assert_eq!(app.answer, "", "one live answer at a time");
        assert!(app.error.is_none());
        assert!(matches!(app.status, Status::Searching));
    }

    #[test]
    fn events_accumulate_tokens_and_finish_idle() {
        let mut app = on_query_screen();
        app.apply(Ok(QueryEvent::Searching));
        assert!(app.is_busy());
        app.apply(Ok(QueryEvent::Snippets(vec![])));
        app.apply(Ok(QueryEvent::Token("Call ".into())));
        app.apply(Ok(QueryEvent::Token("search".into())));
        assert_eq!(app.answer, "Call search");
        app.apply(Ok(QueryEvent::Done { prompt_tokens: 44, completion_tokens: 3 }));
        assert!(!app.is_busy(), "a finished query unlocks the input line");
    }

    #[test]
    fn a_failed_query_shows_the_message_and_unlocks_the_input_line() {
        let mut app = on_query_screen();
        app.apply(Ok(QueryEvent::Searching));
        app.apply(Err(QueryError::Retrieval("embedding server is down".into())));
        assert_eq!(app.error.as_deref(), Some("embedding server is down"));
        assert!(!app.is_busy(), "a failure must not leave the interface stuck");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qemer-tui app`
Expected: FAIL to compile — `App`, `Screen`, `Status`, and `Action` do not exist.

- [ ] **Step 3: Write the implementation**

Prepend to `qemer-tui/src/app.rs`.

```rust
//! Application state, and what a key does to it.
//!
//! `handle_key` mutates state and returns an `Action` describing what the
//! caller should do about the outside world. Returning an intent rather than
//! performing I/O is what lets every key rule be tested without a runtime or
//! a terminal.

use crate::query::{QueryError, QueryEvent};
use qemer_core::{Corpus, Snippet};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Picker,
    Query,
}

/// Where a query has got to. `Searching` and `Streaming` both block input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Searching,
    Streaming,
    Done {
        prompt_tokens: usize,
        completion_tokens: usize,
    },
}

/// What the event loop should do about the outside world.
#[derive(Debug)]
pub enum Action {
    None,
    Quit,
    StartQuery(String),
    Abort,
}

pub struct App {
    pub screen: Screen,
    pub corpora: Vec<Corpus>,
    /// Kept as a plain index rather than a `ListState` because ratatui's
    /// `select_next` does not clamp at the end of the list. Owning the
    /// arithmetic is what makes the bounds testable.
    pub selected: usize,
    pub input: String,
    pub answer: String,
    pub sources: Vec<Snippet>,
    pub status: Status,
    pub error: Option<String>,
}

impl App {
    pub fn new(corpora: Vec<Corpus>) -> Self {
        Self {
            screen: Screen::Picker,
            corpora,
            selected: 0,
            input: String::new(),
            answer: String::new(),
            sources: Vec::new(),
            status: Status::Idle,
            error: None,
        }
    }

    pub fn selected_corpus(&self) -> Option<&Corpus> {
        self.corpora.get(self.selected)
    }

    /// Whether a query is in flight. Input is blocked exactly when this is
    /// true, which is why there is never a second stream.
    pub fn is_busy(&self) -> bool {
        matches!(self.status, Status::Searching | Status::Streaming)
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> Action {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Action::Quit;
        }
        match self.screen {
            Screen::Picker => self.handle_picker_key(key),
            Screen::Query => self.handle_query_key(key),
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> Action {
        match key.code {
            KeyCode::Char('q') => return Action::Quit,
            KeyCode::Down | KeyCode::Char('j') => {
                let last = self.corpora.len().saturating_sub(1);
                self.selected = (self.selected + 1).min(last);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Enter => {
                if self.selected_corpus().is_some() {
                    self.screen = Screen::Query;
                    self.reset_answer();
                    self.input.clear();
                }
            }
            _ => {}
        }
        Action::None
    }

    fn handle_query_key(&mut self, key: KeyEvent) -> Action {
        // Esc is overloaded, resolved by state: it abandons a running query,
        // or leaves the screen when there is nothing to abandon.
        if key.code == KeyCode::Esc {
            if self.is_busy() {
                self.status = Status::Idle;
                return Action::Abort;
            }
            self.screen = Screen::Picker;
            return Action::None;
        }
        if self.is_busy() {
            return Action::None;
        }
        match key.code {
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                let query = self.input.trim().to_string();
                if query.is_empty() {
                    return Action::None;
                }
                self.reset_answer();
                self.status = Status::Searching;
                return Action::StartQuery(query);
            }
            _ => {}
        }
        Action::None
    }

    /// One live answer at a time: asking again replaces what was there.
    fn reset_answer(&mut self) {
        self.answer.clear();
        self.sources.clear();
        self.error = None;
        self.status = Status::Idle;
    }

    /// Fold one stream item into the state.
    pub fn apply(&mut self, event: Result<QueryEvent, QueryError>) {
        match event {
            Ok(QueryEvent::Searching) => self.status = Status::Searching,
            Ok(QueryEvent::Snippets(snippets)) => {
                self.sources = snippets;
                self.status = Status::Streaming;
            }
            Ok(QueryEvent::Token(text)) => {
                self.status = Status::Streaming;
                self.answer.push_str(&text);
            }
            Ok(QueryEvent::Done { prompt_tokens, completion_tokens }) => {
                self.status = Status::Done { prompt_tokens, completion_tokens };
            }
            Err(error) => {
                // Idle rather than Done: a failure must unlock the input line
                // so the user can retype, which is the whole point of being
                // able to escape a bad answer.
                self.error = Some(error.to_string());
                self.status = Status::Idle;
            }
        }
    }
}
```

- [ ] **Step 4: Register the module**

Add `mod app;` to `qemer-tui/src/main.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p qemer-tui app`
Expected: PASS, 16 tests.

- [ ] **Step 6: Commit**

```bash
git add qemer-tui/src/app.rs qemer-tui/src/main.rs
git commit -m "feat(tui): model application state and key handling

- handle_key mutates state and returns an Action, so every key rule is
  testable without a runtime or a terminal.
- Input is blocked exactly while a query is in flight, which is why
  there is never a second stream to interleave.
- Esc is resolved by state: it abandons a running query, or leaves the
  screen when there is nothing to abandon.
- The selected index is plain arithmetic rather than ListState, which
  does not clamp at the end of the list."
```

---

## Task 5: Rendering

Drawing only. This file holds no state and performs no I/O, which is what keeps the previous task testable.

**Files:**
- Create: `qemer-tui/src/view.rs`
- Modify: `qemer-tui/src/main.rs` (add `mod view;`)

**Interfaces:**
- Consumes: `app::{App, Screen, Status}`.
- Produces: `view::draw(frame: &mut Frame, app: &App)`.

- [ ] **Step 1: Write the failing tests**

Create `qemer-tui/src/view.rs` with only the test module. These use `TestBackend` sparingly, for the two things that genuinely cannot be asserted another way.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use qemer_core::{Corpus, CorpusRef};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn a_corpus() -> Corpus {
        Corpus {
            reference: CorpusRef {
                library: "lancedb".into(),
                version: "0.37.1".into(),
                url: "https://example/x.tar.zst".into(),
                sha256: "abc".into(),
                bytes: 1,
                embedding_model: "nomic-embed-text-v1.5".into(),
                embedding_dim: 768,
                snippet_count: 4213,
            },
            path: std::path::PathBuf::from("/tmp/x"),
        }
    }

    fn rendered(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn the_picker_shows_each_installed_corpus() {
        let screen = rendered(&App::new(vec![a_corpus()]));
        assert!(screen.contains("lancedb"), "{screen}");
        assert!(screen.contains("0.37.1"), "{screen}");
    }

    /// A fresh user has no corpora and this slice offers no way in the
    /// interface to get one, so the empty state must name the exact command.
    #[test]
    fn the_empty_picker_names_the_install_command() {
        let screen = rendered(&App::new(vec![]));
        assert!(screen.contains("qemer install"), "{screen}");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p qemer-tui view`
Expected: FAIL to compile — `draw` does not exist.

- [ ] **Step 3: Write the implementation**

Prepend to `qemer-tui/src/view.rs`.

```rust
//! Rendering. Takes `&App`, draws, and holds nothing.
//!
//! Keeping state out of this file is what lets every decision worth
//! asserting live somewhere a test can reach without a terminal.

use crate::app::{App, Screen, Status};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap};

pub fn draw(frame: &mut Frame, app: &App) {
    match app.screen {
        Screen::Picker => draw_picker(frame, app),
        Screen::Query => draw_query(frame, app),
    }
}

fn draw_picker(frame: &mut Frame, app: &App) {
    let area = frame.area();

    if app.corpora.is_empty() {
        let text = vec![
            Line::from("No corpora installed."),
            Line::from(""),
            Line::from("Install one from the command line, then start qemer again:"),
            Line::from(""),
            Line::from("    qemer install <library>@<version>"),
            Line::from(""),
            Line::from("    qemer list    shows what is already installed"),
        ];
        let block = Block::default().borders(Borders::ALL).title(" Qemer ");
        frame.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let items: Vec<ListItem> = app
        .corpora
        .iter()
        .map(|corpus| {
            ListItem::new(Line::from(format!(
                "{} {} · {} snippets",
                corpus.reference.library,
                corpus.reference.version,
                corpus.reference.snippet_count
            )))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Installed corpora — ↑/↓ to move, Enter to open, q to quit ");
    let list = List::new(items)
        .block(block)
        .highlight_symbol("▸ ")
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut state = ListState::default().with_selected(Some(app.selected));
    frame.render_stateful_widget(list, area, &mut state);
}

fn draw_query(frame: &mut Frame, app: &App) {
    let [header, body, sources, input] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(sources_height(app)),
        Constraint::Length(3),
    ])
    .areas(frame.area());

    let title = match app.selected_corpus() {
        Some(corpus) => format!(
            " {} {} ",
            corpus.reference.library, corpus.reference.version
        ),
        None => " qemer ".to_string(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            title,
            Style::default().add_modifier(Modifier::REVERSED),
        ))),
        header,
    );

    // The generated answer is the primary content, with the sources beneath
    // it. If the sources-first fallback in docs/decisions.md is ever taken,
    // this is the weighting that changes.
    let answer_text = if let Some(error) = &app.error {
        error.clone()
    } else if app.answer.is_empty() {
        match app.status {
            Status::Searching => "Searching…".to_string(),
            Status::Streaming => String::new(),
            _ => "Ask a question about this library.".to_string(),
        }
    } else {
        app.answer.clone()
    };
    let answer_block = Block::default().borders(Borders::ALL).title(status_title(app));
    frame.render_widget(
        Paragraph::new(answer_text)
            .block(answer_block)
            .wrap(Wrap { trim: false }),
        body,
    );

    if !app.sources.is_empty() {
        let lines: Vec<Line> = app
            .sources
            .iter()
            .enumerate()
            .map(|(i, snippet)| {
                let url = snippet.source_url.as_deref().unwrap_or("");
                Line::from(format!("{}. {}  {}", i + 1, snippet.title, url))
            })
            .collect();
        let block = Block::default().borders(Borders::ALL).title(" Sources ");
        frame.render_widget(Paragraph::new(lines).block(block), sources);
    }

    let prompt = if app.is_busy() {
        " Esc to abort ".to_string()
    } else {
        " Ask — Enter to send, Esc for the corpus list ".to_string()
    };
    let input_block = Block::default().borders(Borders::ALL).title(prompt);
    frame.render_widget(
        Paragraph::new(app.input.as_str()).block(input_block),
        input,
    );
}

/// Two lines of chrome plus one per source, capped so a long list cannot
/// squeeze the answer off the screen.
fn sources_height(app: &App) -> u16 {
    if app.sources.is_empty() {
        return 0;
    }
    (app.sources.len() as u16 + 2).min(9)
}

fn status_title(app: &App) -> String {
    match app.status {
        Status::Idle => " Answer ".to_string(),
        Status::Searching => " Answer — searching ".to_string(),
        Status::Streaming => " Answer — generating, Esc to abort ".to_string(),
        Status::Done { prompt_tokens, completion_tokens } => {
            // Zeroes mean the server reported no usage, not that nothing
            // happened; saying so beats showing "0 tokens".
            if prompt_tokens == 0 && completion_tokens == 0 {
                " Answer — complete ".to_string()
            } else {
                format!(" Answer — {prompt_tokens} prompt, {completion_tokens} generated ")
            }
        }
    }
}
```

- [ ] **Step 4: Register the module**

Add `mod view;` to `qemer-tui/src/main.rs`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p qemer-tui view`
Expected: PASS, 2 tests.

If `Layout::vertical(...).areas(...)` does not destructure into an array, check the constraint count matches the array length exactly — `areas::<4>` infers its size from the binding.

- [ ] **Step 6: Commit**

```bash
git add qemer-tui/src/view.rs qemer-tui/src/main.rs
git commit -m "feat(tui): render the picker and query screens

- The answer is the primary content with sources beneath it, which is
  the weighting docs/decisions.md records; the sources-first fallback
  would change this file and the prompt, not the structure.
- The empty picker names the exact install command, since this slice
  offers no way in the interface to get a corpus.
- Rendering holds no state and does no I/O, which is what keeps the
  state transitions testable without a terminal."
```

---

## Task 6: The event loop

Joins everything. Little of this is unit-testable, which is why the previous five tasks pushed every rule somewhere that is.

**Files:**
- Modify: `qemer-tui/src/app.rs` (add the loop)
- Modify: `qemer-tui/src/main.rs`
- Modify: `qemer-tui/Cargo.toml`

**Interfaces:**
- Consumes: everything above.
- Produces: `app::run(terminal: &mut DefaultTerminal, config: &Config, corpora: Vec<Corpus>) -> Result<()>`.

- [ ] **Step 1: Fix the crossterm dependency**

`EventStream` is behind a non-default feature, so the loop below does not compile until this changes. In `qemer-tui/Cargo.toml`, replace the existing crossterm line with:

```toml
# Declared only to enable `event-stream`, which is not a default feature and
# which neither ratatui nor ratatui-crossterm forwards. Every `use` goes
# through `ratatui::crossterm` so the types stay identical to the ones
# ratatui builds against — the same rule docs/decisions.md sets for Arrow.
crossterm = { version = "0.29.0", default-features = false, features = ["event-stream"] }
```

- [ ] **Step 2: Confirm one crossterm remains in the graph**

Run: `cargo tree -p qemer-tui -i crossterm`
Expected: a single `crossterm v0.29.0`, depended on by both `qemer-tui` and `ratatui-crossterm`. If two versions appear, stop — that is exactly the skew this arrangement exists to prevent.

- [ ] **Step 3: Write the event loop**

Append to `qemer-tui/src/app.rs`, above the test module.

```rust
use crate::config::Config;
use crate::{query, view};
use color_eyre::Result;
use futures::StreamExt;
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, EventStream};

/// Run until the user quits.
///
/// The whole query lives in one `Option<Stream>`. Aborting is setting it to
/// `None`: the drop closes the connection, which stops the server generating
/// and unlocks the input line. That is the entire cancellation story, and it
/// is why no abort API was needed from either library crate.
pub async fn run(
    terminal: &mut DefaultTerminal,
    config: &Config,
    corpora: Vec<Corpus>,
) -> Result<()> {
    let mut app = App::new(corpora);
    let mut keys = EventStream::new();
    let mut running: Option<std::pin::Pin<Box<dyn futures::Stream<Item = Result<QueryEvent, QueryError>> + Send>>> = None;

    loop {
        terminal.draw(|frame| view::draw(frame, &app))?;

        let action = if let Some(stream) = running.as_mut() {
            tokio::select! {
                event = keys.next() => handle_terminal_event(&mut app, event),
                item = stream.next() => {
                    match item {
                        Some(item) => {
                            app.apply(item);
                            Action::None
                        }
                        // The stream ended on its own; there is nothing left
                        // to hold.
                        None => {
                            running = None;
                            Action::None
                        }
                    }
                }
            }
        } else {
            let event = keys.next().await;
            handle_terminal_event(&mut app, event)
        };

        match action {
            Action::Quit => return Ok(()),
            Action::Abort => running = None,
            Action::StartQuery(text) => {
                let Some(corpus) = app.selected_corpus() else {
                    continue;
                };
                // Cloned so the stream owns everything and borrows no part of
                // `app`, which the loop keeps mutating.
                let corpus = qemer_core::Corpus {
                    reference: corpus.reference.clone(),
                    path: corpus.path.clone(),
                };
                running = Some(Box::pin(query::run(
                    corpus,
                    config.embed_client(),
                    config.generator(),
                    text,
                    config.retrieval.k,
                )));
            }
            Action::None => {}
        }
    }
}

fn handle_terminal_event(
    app: &mut App,
    event: Option<std::io::Result<Event>>,
) -> Action {
    match event {
        Some(Ok(Event::Key(key))) if key.is_press() => app.handle_key(key),
        // A closed terminal event stream means the terminal went away.
        None => Action::Quit,
        _ => Action::None,
    }
}
```

Add `use crate::query::{QueryError, QueryEvent};` to the existing imports if it is not already there, and `use qemer_core::Corpus;` likewise.

- [ ] **Step 4: Wire up `main`**

Replace `qemer-tui/src/main.rs`:

```rust
//! Qemer TUI: pick a library, ask a question, read the sources and the answer.

mod app;
mod cli;
mod config;
mod query;
mod view;

use clap::Parser;
use color_eyre::Result;
use qemer_core::Cache;

#[derive(Debug, clap::Parser)]
#[command(name = "qemer", about = "Offline coding help grounded in documentation")]
struct Args {
    #[command(subcommand)]
    command: Option<cli::Command>,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    // Parsed before the config is read, so `--help` and `--version` work on a
    // machine that has never been configured.
    let args = Args::parse();
    let config = config::load()?;

    match args.command {
        Some(cli::Command::Install { target }) => return cli::install(&config, &target).await,
        Some(cli::Command::List) => return cli::list(&config),
        None => {}
    }

    let cache = Cache::new(Cache::default_root()?);
    let corpora = cache.installed()?;

    // Restore the terminal even on a panic. Without this, a crash leaves the
    // shell in raw mode with no echo, which looks like a hung machine.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        hook(info);
    }));

    let mut terminal = ratatui::init();
    let outcome = app::run(&mut terminal, &config, corpora).await;
    ratatui::restore();
    outcome
}
```

- [ ] **Step 5: Build and check the whole workspace**

Run: `cargo check --workspace`
Expected: PASS.

Run: `cargo clippy -p qemer-tui --all-targets`
Expected: no warnings.

Run: `cargo test -p qemer-tui`
Expected: PASS, 36 tests.

- [ ] **Step 6: Check the boundaries held**

Run: `grep -rn "^use crossterm\|[^:]use crossterm::" qemer-tui/src/`
Expected: no hits. Every crossterm import goes through `ratatui::crossterm`.

Run: `grep -rniE "browse|evict|newest|latest version" qemer-tui/src/`
Expected: no hits. This slice resolves none of the deferred questions.

- [ ] **Step 7: Verify by hand**

This is the one part no test covers, so run it.

```bash
QEMER_CONFIG=/tmp/qemer-test.toml cargo run -p qemer-tui
```

With `/tmp/qemer-test.toml` absent, expect the missing-config error naming that path. With it present but lacking `context_tokens`, expect the message naming that key and pointing at the llama-server startup log. With a complete config and no corpora installed, expect the empty picker naming the install command, and `q` to exit cleanly with the terminal restored.

- [ ] **Step 8: Commit**

```bash
git add qemer-tui/src/app.rs qemer-tui/src/main.rs qemer-tui/Cargo.toml Cargo.lock
git commit -m "feat(tui): drive the query loop from a single event loop

- One Option<Stream> holds the whole query; aborting is dropping it,
  which closes the connection and stops the server generating.
- Enable crossterm's event-stream feature, which is not a default and
  which neither ratatui nor ratatui-crossterm forwards; imports still
  go through ratatui's re-export so the types cannot skew.
- Restore the terminal from a panic hook, since a crash in raw mode
  looks to the user like a hung machine.
- Arguments are parsed before config is read, so --help works on a
  machine that has never been configured."
```

---

## What this plan does not cover

Deliberately out of scope, each needing its own design.

- **Corpus browsing, version selection, and cache eviction.** Open questions in `docs/decisions.md`, and browsing additionally needs `corpus::install()` to grow a streaming download with progress reporting, since it currently buffers the whole tarball with `.bytes().await`.
- **Scrolling a long answer or a long source list.** The answer pane wraps but does not scroll; a long answer runs off the bottom. Worth adding once there is a real corpus to produce one.
- **Multi-turn conversation.** Requires prompt assembly to carry history, which re-opens budgeting.
- **Whether `k = 5` is any good.** A placeholder with no evidence behind it, as `docs/decisions.md` says.

## Open questions this plan must not answer on its own

If executing this plan appears to require resolving any of these, **stop and ask.**

- **What `context_tokens` should be.** No number is proposed anywhere, and a test asserts that no error message suggests one. Reading it from the running server was considered and rejected during design; see the spec.
- **Which corpus version to install when the user does not say.** The version argument is required for exactly this reason.
- **What the manifest URL is.** No host has been chosen; the key is required and has no default.
- **Whether `llama-server` needs a per-request `model` field.** `docs/decisions.md` lists this as unverified. Both crates send one already; nothing here changes that.
