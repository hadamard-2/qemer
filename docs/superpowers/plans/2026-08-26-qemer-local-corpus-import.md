# Qemer Local Corpus Import Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Qemer discover and install explicitly versioned corpora from a local or HTTPS manifest, and configure its local model endpoints through a guided command, while keeping source ingestion outside this repository.

**Architecture:** `qemer-core` receives a manifest location, resolves each artifact location relative to that manifest, and imports verified bytes into the existing cache under its `(library, version)` identity. `qemer-tui` owns the command-line policy: it lists available versions, requires an explicit version for installation, keeps corpus commands independent of model configuration, and provides `qemer config` as the guided writer for query settings.

**Tech Stack:** Rust 2024, Clap, Tokio, Reqwest, `url`, Dialoguer, LanceDB, Ratatui, existing workspace tests.

**Spec:** `docs/superpowers/specs/2026-08-26-qemer-local-corpus-import-design.md`

## Global Constraints

- `qemer-core` must never depend on `qemer-answer` or refer to generation, prompts, completion, or model-runtime management.
- `qemer-answer` never retrieves; this plan does not modify it.
- Qemer must not accept source repository/documentation URLs or invoke `qemer-ingest`; it consumes only a manifest and corpus artifact.
- The manifest/tarball/Parquet contract remains the sole Qemer-to-ingest integration surface.
- `qemer install` accepts a local manifest path or an HTTPS manifest URL; it rejects other schemes.
- A manifest may contain many versions of one library; only duplicate `(library, version)` pairs are invalid.
- `qemer install` requires an explicit `library@version`; `qemer available` is the discovery command for a supplied manifest.
- `qemer config` writes only Qemer's existing XDG config file, atomically, and never installs or starts a model runtime.
- Do not commit or push while executing this plan unless the user explicitly asks in that execution turn.

## File Structure

| File | Responsibility |
| --- | --- |
| `docs/decisions.md` | Record the local-or-HTTPS artifact-consumer boundary and versioned-manifest rule. |
| `qemer-core/Cargo.toml` | Add the URL parser used to validate and resolve manifest/artifact locations. |
| `qemer-core/src/corpus.rs` | Load manifests from a local path or HTTPS, resolve artifacts, validate a manifest, read local bytes or download HTTPS bytes, then reuse the existing verification/install path. |
| `qemer-core/src/lib.rs` | Add distinct errors for invalid source locations, local reads, invalid manifests, and embedding transport versus response failures. |
| `qemer-core/tests/local_import.rs` | Exercise a complete local manifest-to-cache import without a network listener. |
| `qemer-tui/src/cli.rs` | Define config-free `available`, versioned `install`, `list`, and `config` command dispatch. |
| `qemer-tui/src/main.rs` | Dispatch non-TUI commands before loading query configuration and expose a real Clap version string. |
| `qemer-tui/Cargo.toml` | Publish the executable as `qemer`. |
| `qemer-tui/src/config.rs` | Remove the now-invalid global manifest URL setting and provide validated, atomic config writing for the wizard. |
| `qemer-tui/src/config_wizard.rs` | Prompt for and save existing/new query configuration without exposing its filesystem path as the primary UX. |
| `qemer-tui/src/query.rs` | Give server-start advice only for transport failures. |
| `qemer-tui/src/view.rs` | Render the new install hint and a visible editable cursor. |
| `README.md` | Describe the local artifact workflow, endpoint configuration, and the lack of an embedded ingestion/crawler feature. |

### Task 1: Record the consumer contract and add manifest-source validation

**Files:**

- Modify: `docs/decisions.md`
- Modify: `qemer-core/Cargo.toml`
- Modify: `qemer-core/src/lib.rs`
- Modify: `qemer-core/src/corpus.rs`

**Interfaces:**

- Produces `corpus::load_manifest(source: &str) -> Result<Manifest>`.
- Produces `corpus::find_corpus(manifest: Manifest, library: &str, version: &str) -> Result<CorpusRef>`.
- Produces a `CorpusRef` whose `url` is an absolute HTTPS URL or absolute local file path before `corpus::install` receives it.

- [ ] **Step 1: Amend the decisions document before code changes.**

Replace the R2/GitHub-hosting sentence under “Prebuilt corpora, not local ingestion” with a statement that Qemer imports prebuilt artifacts from an explicit local or HTTPS manifest source, and add a settled decision that the manifest identity is `(library, version)`. Move catalog browsing, update policy, and hosting out of the current scope rather than describing a default host.

- [ ] **Step 2: Add the failing manifest-resolution tests in `qemer-core/src/corpus.rs`.**

Add tests with the following exact behavioural assertions:

```rust
#[test]
fn a_relative_artifact_resolves_beside_a_local_manifest() {
    let source = ManifestSource::parse("/tmp/corpora/manifest.json").unwrap();
    let resolved = source.resolve_artifact("numpy-2.3.0.tar.zst").unwrap();
    assert_eq!(resolved, ArtifactSource::File("/tmp/corpora/numpy-2.3.0.tar.zst".into()));
}

#[test]
fn a_relative_artifact_resolves_against_an_https_manifest() {
    let source = ManifestSource::parse("https://host.example/releases/manifest.json").unwrap();
    let resolved = source.resolve_artifact("numpy-2.3.0.tar.zst").unwrap();
    assert_eq!(resolved.to_string(), "https://host.example/releases/numpy-2.3.0.tar.zst");
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
fn a_non_https_remote_manifest_is_rejected() {
    assert!(ManifestSource::parse("ftp://host.example/manifest.json").is_err());
}
```

- [ ] **Step 3: Run the new tests and confirm they fail because the source types do not exist.**

Run: `cargo test -p qemer-core corpus::tests -- --nocapture`

Expected: compilation failure mentioning `ManifestSource` and `ArtifactSource`.

- [ ] **Step 4: Implement source parsing, resolution, and manifest validation.**

Add `url = "2"` to `qemer-core/Cargo.toml`. In `qemer-core/src/corpus.rs`, introduce these types and keep them private to the corpus module:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
enum ManifestSource {
    File(std::path::PathBuf),
    Https(url::Url),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ArtifactSource {
    File(std::path::PathBuf),
    Https(url::Url),
}
```

`ManifestSource::parse` treats an existing/non-URL argument as a filesystem path, accepts only `https` URLs as remote sources, and rejects every other explicit URL scheme with `CoreError::Manifest`. `resolve_artifact` accepts a relative path, an absolute HTTPS URL, or an absolute local path; it rejects `file://`, `http://`, and every other scheme. It uses `Path::parent` for local manifests and `Url::join` for HTTPS manifests. Convert the resolved result back into the persisted `CorpusRef.url` string after validation.

Make `parse_manifest` call `validate_manifest(&Manifest)`, where validation inserts `(library, version)` pairs into a `HashSet` and returns `CoreError::Manifest` on a duplicate. Add `find_corpus` that returns the one entry matching both library and version or a `CoreError::CorpusMissing(format!("{library}@{version}"))` error. Do not add any ingest-specific metadata or code.

- [ ] **Step 5: Add and run focused tests for the completed contract.**

Run: `cargo test -p qemer-core corpus::tests -- --nocapture`

Expected: PASS, including local/HTTPS relative resolution, duplicate `(library, version)` rejection, multiple-version acceptance, malformed JSON, and checksum tests.

- [ ] **Step 6: Inspect the boundary before proceeding.**

Run: `rg -n -i "qemer-ingest|github|crawl|scrap|completion|prompt|answer" qemer-core/src`

Expected: only the existing generic corpus-contract module comment may mention `qemer-ingest`; no source acquisition or generation implementation may appear.

### Task 2: Import verified local or HTTPS artifacts into the existing cache

**Files:**

- Modify: `qemer-core/src/corpus.rs`
- Modify: `qemer-core/src/lib.rs`
- Create: `qemer-core/tests/local_import.rs`

**Interfaces:**

- Consumes `ManifestSource`, `ArtifactSource`, and normalized `CorpusRef` from Task 1.
- Produces `corpus::load_manifest(source: &str) -> Result<Manifest>`, `corpus::find_corpus`, and retains `corpus::install(cache, reference) -> Result<Corpus>`.

- [ ] **Step 1: Write a local import integration test.**

Create `qemer-core/tests/local_import.rs`. Reuse `fixture::write_fixture_parquet` by making the fixture module public to integration tests or by moving the shared fixture to `qemer-core/tests/support/mod.rs`. The test must create `corpus.parquet`, tar/zstd it as `numpy-2.3.0.tar.zst`, compute its SHA-256 and byte length, and write this sibling `manifest.json`:

```json
{
  "corpora": [{
    "library": "numpy",
    "version": "2.3.0",
    "url": "numpy-2.3.0.tar.zst",
    "sha256": "<computed-by-test>",
    "bytes": <computed-by-test>,
    "embedding_model": "nomic-embed-text-v1.5",
    "embedding_dim": 768,
    "snippet_count": 3
  }]
}
```

Then load it and install it:

```rust
let manifest = qemer_core::corpus::load_manifest(manifest_path.to_str().unwrap()).await.unwrap();
let reference = qemer_core::corpus::find_corpus(manifest, "numpy", "2.3.0").unwrap();
let installed = qemer_core::corpus::install(&cache, &reference).await.unwrap();
assert!(installed.path.join("corpus.json").is_file());
assert_eq!(cache.installed().unwrap()[0].reference.library, "numpy");
```

- [ ] **Step 2: Run the integration test and confirm it fails because the manifest loader is absent.**

Run: `cargo test -p qemer-core --test local_import -- --nocapture`

Expected: compilation failure mentioning `load_manifest`.

- [ ] **Step 3: Implement local and HTTPS byte readers.**

Implement the following public entry point in `qemer-core/src/corpus.rs`:

```rust
pub async fn load_manifest(source: &str) -> Result<Manifest> {
    let source = ManifestSource::parse(source)?;
    let bytes = source.read_bytes().await?;
    let mut manifest = parse_manifest(&bytes)?;
    for reference in &mut manifest.corpora {
        reference.url = source.resolve_artifact(&reference.url)?.to_string();
    }
    Ok(manifest)
}
```

`read_bytes` uses `std::fs::read` for `File` and the existing Reqwest `error_for_status` path for `Https`. Add dedicated `CoreError` variants so file-read failures name the filesystem path and network failures name the HTTPS URL. In `install`, replace the unconditional `reqwest::get` with `ArtifactSource::parse(&reference.url)?.read_bytes().await?`, then check `bytes.len() as u64 == reference.bytes` before `verify_sha256`. Return a new `CoreError::SizeMismatch` with expected and actual byte counts on failure.

- [ ] **Step 4: Add the failure-path tests.**

Add tests proving a manifest with a relative artifact that is missing from disk returns an error naming that artifact path, and that a manifest whose advertised `bytes` differs from the local tarball is rejected before unpacking. Use a temporary directory and no socket server.

- [ ] **Step 5: Run the core verification suite.**

Run: `cargo test -p qemer-core --lib --tests`

Expected: PASS. The new local import test must prove the exact manifest/artifact contract independent of `qemer-ingest` implementation details.

### Task 3: Replace the catalog-dependent CLI with explicit manifest import

**Files:**

- Modify: `qemer-tui/Cargo.toml`
- Modify: `qemer-tui/src/main.rs`
- Modify: `qemer-tui/src/cli.rs`
- Modify: `qemer-tui/src/config.rs`

**Interfaces:**

- Consumes `corpus::load_manifest`, `corpus::find_corpus`, and `corpus::install` from Tasks 1–2.
- Produces `qemer available --manifest <path-or-https-url>`, `qemer install <library>@<version> --manifest <path-or-https-url>`, and config-free `qemer list`.

- [ ] **Step 1: Write failing CLI parser tests in `qemer-tui/src/cli.rs`.**

Keep and extend the existing `library@version` parser tests so the target remains explicit:

```rust
#[test]
fn a_target_splits_into_library_and_version() {
    assert_eq!(parse_target("numpy@2.3.0").unwrap(), ("numpy".into(), "2.3.0".into()));
}

#[test]
fn a_target_without_a_version_is_rejected() {
    assert!(parse_target("numpy").is_err());
}
```

Add a `clap::CommandFactory` test for the subcommand shape:

```rust
#[test]
fn available_and_install_require_a_manifest_option() {
    let command = crate::Args::command();
    for name in ["available", "install"] {
        let subcommand = command.get_subcommands().find(|command| command.get_name() == name).unwrap();
        assert!(subcommand.get_arguments().any(|argument| argument.get_long() == Some("manifest")));
    }
}
```

Make `Args` visible to this module with `pub(crate)` visibility.

- [ ] **Step 2: Run the CLI tests and confirm they fail against the old `library@version` command.**

Run: `cargo test -p qemer-tui cli::tests -- --nocapture`

Expected: FAIL because `available` and the required `--manifest` option do not exist.

- [ ] **Step 3: Implement the new command surface.**

Replace the install variant with:

```rust
Install {
    target: String,
    #[arg(long)]
    manifest: String,
}
```

Add `Available { #[arg(long)] manifest: String }`. `cli::available` loads the manifest, sorts entries by `(library, version)`, and prints one `library@version · N snippets · N bytes` line per entry. `cli::install` parses `target`, calls `corpus::load_manifest(&manifest).await`, then `corpus::find_corpus(manifest, &library, &version)`, prints the resolved library/version/snippet count, and calls the existing install function. Change `cli::list` to take no `Config` parameter.

In `main`, dispatch `Available`, `Install`, `List`, and `Config` before `config::load()`. Add `version` to the Clap `#[command(...)]` attribute. In `qemer-tui/Cargo.toml`, add:

```toml
[[bin]]
name = "qemer"
path = "src/main.rs"
```

Remove `manifest_url` from `RawConfig`, `Config`, parsing, and config tests. A missing config must now mention only query-time model settings, never corpus hosting.

- [ ] **Step 4: Run focused command checks.**

Run: `cargo test -p qemer-tui cli::tests config::tests -- --nocapture && cargo run -p qemer-tui --bin qemer -- --version && cargo run -p qemer-tui --bin qemer -- list`

Expected: the unit tests pass; `--version` prints the package version; `list` reports `no corpora installed` without requiring `QEMER_CONFIG`.

- [ ] **Step 5: Verify binary naming in release metadata.**

Run: `cargo metadata --no-deps --format-version 1 | rg '"name":"qemer"|"name":"qemer-tui"'`

Expected: package metadata still identifies the crate as `qemer-tui`, while the declared binary target is named `qemer`.

### Task 4: Add the guided `qemer config` command

**Files:**

- Modify: `qemer-tui/Cargo.toml`
- Modify: `qemer-tui/src/cli.rs`
- Modify: `qemer-tui/src/config.rs`
- Create: `qemer-tui/src/config_wizard.rs`
- Modify: `qemer-tui/src/main.rs`

**Interfaces:**

- Produces `config::ConfigDraft`, `config::validate_draft(draft: ConfigDraft) -> Result<Config, ConfigError>`, and `config::write(path: &Path, config: &Config) -> Result<(), ConfigError>`.
- Produces `config_wizard::run() -> color_eyre::Result<()>`, invoked only by `qemer config`.

- [ ] **Step 1: Write pure configuration tests before adding prompts.**

Add tests in `qemer-tui/src/config.rs` for a valid draft, a zero embedding dimension, a zero context length, a zero completion limit, a completion limit greater than its context length, and an atomic write/read round trip in a temporary directory. The round trip must assert that the written text re-parses to the same endpoint URLs, models, dimensions, and token limits.

- [ ] **Step 2: Run the configuration tests and confirm the draft/writer API is absent.**

Run: `cargo test -p qemer-tui config::tests -- --nocapture`

Expected: compilation failure mentioning `ConfigDraft`, `validate_draft`, and `write`.

- [ ] **Step 3: Implement the validated atomic writer.**

Use `cargo add dialoguer tempfile -p qemer-tui` and retain the resolved versions in `Cargo.lock`. Add `ConfigError::Unwritable { path, reason }` for directory creation, temporary-file, serialization, and persist failures. Add a serializable `ConfigDraft` with the existing embedding, completion, and retrieval values. `validate_draft` must reject zero numeric values and a completion limit greater than its context window, then return the existing `Config` type. `write` must create the parent config directory, serialize TOML to a `tempfile::NamedTempFile` in that directory, and persist it to the path returned by `resolve_path`; it must never truncate the existing config before the replacement is fully written.

- [ ] **Step 4: Implement the wizard and command dispatch.**

In `config_wizard::run`, call `resolve_path(std::env::var("QEMER_CONFIG").ok())`. If the file is absent, use the existing built-in defaults for endpoint URLs, models, dimensions, and retrieval `k`, but require the user to enter both completion token values because Qemer deliberately has no safe defaults for those model-dependent limits. If the file is valid, use its values as each prompt's default; if it is malformed or unreadable, show the existing error and do not overwrite it. Prompt with `dialoguer::Input` for `embedding.base_url`, `embedding.model`, `embedding.dim`, `completion.base_url`, `completion.model`, `completion.context_tokens`, and `completion.max_completion_tokens`. Validate the assembled draft before calling `write` and print the saved config path on success. Add `Config` as a `cli::Command` variant and dispatch it before normal config loading.

- [ ] **Step 5: Run the focused configuration checks.**

Run: `cargo test -p qemer-tui config::tests -- --nocapture && cargo run -p qemer-tui --bin qemer -- config --help`

Expected: pure config tests pass and help documents the guided configuration command without opening an interactive prompt.

### Task 5: Polish the TUI’s local-import and failure behaviour

**Files:**

- Modify: `qemer-core/src/lib.rs`
- Modify: `qemer-core/src/embed.rs`
- Modify: `qemer-tui/src/query.rs`
- Modify: `qemer-tui/src/view.rs`
- Modify: `qemer-tui/src/app.rs` only if a focused-input flag is needed for rendering tests

**Interfaces:**

- Consumes `CoreError::EmbedUnreachable { url, reason }` and `CoreError::EmbedResponse(String)`.
- Produces a visible query cursor and a no-corpora message that names the local manifest import command.

- [ ] **Step 1: Write the failing error-classification tests in `qemer-tui/src/query.rs`.**

Replace the broad `CoreError::Embed` test setup with this pair:

```rust
#[test]
fn an_unreachable_embedding_server_names_that_endpoint_and_what_to_start() {
    let error = CoreError::EmbedUnreachable {
        url: EMBEDDING_URL.into(),
        reason: "connection refused".into(),
    };
    assert!(describe_retrieval_failure(&error, EMBEDDING_URL).contains("Start llama-server"));
}

#[test]
fn a_malformed_embedding_response_does_not_offer_server_start_advice() {
    let message = describe_retrieval_failure(&CoreError::EmbedResponse("invalid JSON".into()), EMBEDDING_URL);
    assert!(message.contains("invalid JSON"));
    assert!(!message.contains("Start llama-server"));
}
```

- [ ] **Step 2: Write the failing render assertions in `qemer-tui/src/view.rs`.**

Change the empty-picker test to assert the exact command fragment `qemer install <library>@<version> --manifest <path-or-https-url>`. Add this query-screen test:

```rust
#[test]
fn an_editable_query_shows_a_visible_cursor() {
    let mut app = App::new(vec![a_corpus()]);
    app.screen = Screen::Query;
    app.input = "how do I create an array".into();
    let screen = rendered(&app);
    assert!(screen.contains("how do I create an array▌"), "{screen}");
}
```

- [ ] **Step 3: Run the focused tests and confirm they fail.**

Run: `cargo test -p qemer-tui 'query::tests|view::tests' -- --nocapture`

Expected: compile/test failures because the new error variants and cursor/import copy are absent.

- [ ] **Step 4: Implement precise embedding errors and rendering.**

Replace `CoreError::Embed(String)` with `EmbedUnreachable { url: String, reason: String }` and `EmbedResponse(String)`. In `EmbedClient::embed`, map only `.send()` failures to `EmbedUnreachable`; map a non-success status, unreadable response body, invalid JSON, empty data, and a dimension mismatch to `EmbedResponse`. Update `parse_embedding` to construct only `EmbedResponse`.

Update `describe_retrieval_failure` to provide llama-server startup advice only for `EmbedUnreachable`; every other error returns `to_string()` unchanged. In `draw_query`, render `format!("{}▌", app.input)` while `!app.is_busy()` and render only `app.input` while a query is running. Replace the empty-state copy with the exact explicit-version manifest command.

- [ ] **Step 5: Run the TUI test suite.**

Run: `cargo test -p qemer-tui`

Expected: PASS. Do not launch a browser for these terminal rendering changes; the Ratatui `TestBackend` tests are the verification surface.

### Task 6: Publish an accurate local-first workflow and run final verification

**Files:**

- Modify: `README.md`
- Modify: `docs/decisions.md` if Task 1 left any now-stale hosting/install wording

**Interfaces:**

- Documents the commands and artifact contract implemented in Tasks 1–5.

- [ ] **Step 1: Replace the stale scaffold language in `README.md`.**

State that Qemer imports prebuilt corpora from an explicit manifest and does not crawl, scrape, embed a corpus, or manage a model runtime. Include this local workflow exactly:

```sh
qemer config
qemer available --manifest /absolute/path/to/manifest.json
qemer install numpy@2.3.0 --manifest /absolute/path/to/manifest.json
qemer list
qemer
```

Explain that the manifest sits beside the `.tar.zst` artifact when its `url` is relative, and that Qemer verifies the advertised byte count and SHA-256 before installing. Retain the two user-run llama-server endpoint requirement, but remove claims that Qemer selects a central host or that the implementations are still `todo!()`.

- [ ] **Step 2: Add an executable command example to the README acceptance check.**

Use a temporary manifest fixture from Task 2 and document in a test or scripted manual check that `qemer available --manifest "$fixture/manifest.json"` lists `numpy@2.3.0`, `qemer install numpy@2.3.0 --manifest "$fixture/manifest.json"` succeeds, and `qemer list` shows `numpy 2.3.0`.

- [ ] **Step 3: Format and run the complete verification set.**

Run: `cargo fmt --check && cargo check --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`

Expected: all commands pass. If formatting fails in unrelated pre-existing files, report those paths separately and do not reformat unrelated code merely to make this task appear clean.

- [ ] **Step 4: Review the final boundary and user workflow.**

Run: `rg -n -i "qemer-ingest|github repository|documentation URL|crawl|scrap|r2|cloudflare|install.*llama" qemer-core qemer-tui README.md docs/decisions.md`

Expected: Qemer may mention the separate ingest contract generically, but contains no source-fetching feature, hosting commitment, or runtime-install promise. The README shows the explicit local-manifest installation workflow.

- [ ] **Step 5: Leave the work unstaged unless the user explicitly requests a commit.**

Run: `git status --short`

Expected: reviewable source and documentation changes, with no `git commit` or `git push` performed by this plan.

## Plan Self-Review

**Spec coverage:** Task 1 records and enforces the manifest-only boundary and versioned-corpus rule. Task 2 implements local/HTTPS import, byte-size verification, checksum verification, and cache installation. Task 3 implements version discovery/import plus config-free corpus commands and binary/version corrections. Task 4 implements the guided configuration path. Task 5 covers the outstanding TUI usability and diagnostic defects. Task 6 documents and verifies the user workflow. No task introduces ingestion, crawling, hosting, automatic runtime management, or a curl installer.

**Placeholder scan:** No task delegates unspecified behaviour. The manifest source schemes, relative-resolution rules, duplicate-pair condition, commands, prompt fields, validation rules, test assertions, error categories, and verification commands are explicit.

**Type consistency:** `load_manifest`, `find_corpus`, and `install` are defined in the core tasks and consumed by the CLI task. `ConfigDraft`, `validate_draft`, and `write` are defined in Task 4 and consumed by its wizard. `EmbedUnreachable` and `EmbedResponse` are defined in Task 5 and consumed by its query/TUI tests. The command shape in the spec matches Tasks 3–6.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-26-qemer-local-corpus-import.md`. This plan should be executed only after `qemer-ingest` has been created as its separate repository or with a temporary fixture, because Qemer intentionally does not depend on its implementation.
