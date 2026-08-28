# Demo Corpus Ingestion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a repository-local, demo-only Rust tool that turns the supplied NumPy and PyTorch Context7 snapshots into Qemer-compatible, release-ready corpus archives and a manifest.

**Architecture:** `tools/qemer-demo-ingest` is an independent nested Cargo workspace, deliberately outside Qemer’s three runtime crates and with no dependency on them. It parses the two fixed-format local snapshots, sends one OpenAI-shaped embedding request per prose or code row to a user-run server, writes the exact Parquet schema Qemer installs, then packages each Parquet file as a `tar.zst` archive and emits a manifest that points at a user-supplied GitHub Release asset base URL.

**Tech Stack:** Rust 2024; clap 4; reqwest 0.13; tokio 1.53; serde/serde_json; Arrow and Parquet 58.4; tar; zstd; sha2.

**Spec:** [`docs/superpowers/specs/2026-08-25-demo-corpus-ingestion-design.md`](../specs/2026-08-25-demo-corpus-ingestion-design.md)

## Global Constraints

- This is a demo-only tool under `tools/`, not a fourth Qemer runtime crate and not a general `qemer-ingest` implementation.
- Read only the two supplied local source files; do not fetch documentation from their original URLs.
- Split only on a separator line of exactly 32 dashes, and fail rather than emit a corpus when that input guarantee is absent or malformed.
- Use the supplied GGUF only through a user-run embedding server; never download, install, or launch model runtime software.
- Require explicit `--embedding-model`, `--embedding-dim`, and `--embedding-url` arguments; stamp model and dimension into every manifest entry.
- Preserve all fenced code in a block by joining multiple fence contents with two newlines into its single optional `code` row.
- Produce only the settled contract: a tar.zst archive containing `corpus.parquet`, plus a JSON manifest containing `library`, `version`, `url`, `sha256`, `bytes`, `embedding_model`, `embedding_dim`, and `snippet_count`.
- Generated archives and manifests are staging artifacts; never add them to Git.
- Do not create a GitHub Release or upload an asset as part of tool execution or tests. Publishing is a separate, explicitly authorized action.
- Do not commit unless the user explicitly asks for a commit.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `tools/qemer-demo-ingest/Cargo.toml` | Independent nested Cargo workspace and exact dependency versions, isolated from the root workspace’s three crates. |
| `tools/qemer-demo-ingest/src/lib.rs` | Public module surface shared by the binary and integration tests. |
| `tools/qemer-demo-ingest/src/error.rs` | Typed parse, embedding, filesystem, Parquet, archive, and manifest errors. |
| `tools/qemer-demo-ingest/src/snapshot.rs` | Strict 32-dash snapshot parser and conversion of one block into prose/code text units. |
| `tools/qemer-demo-ingest/src/embed.rs` | OpenAI-shaped embedding client, response parsing, width validation, and bounded sequential embedding. |
| `tools/qemer-demo-ingest/src/package.rs` | Arrow schema, Parquet writer, tar.zst builder, SHA-256 calculation, and manifest construction. |
| `tools/qemer-demo-ingest/src/main.rs` | Explicit CLI arguments and orchestration of the two corpus builds into an empty staging directory. |
| `tools/qemer-demo-ingest/tests/fixtures/snapshot.txt` | Small representative snapshot containing a 32-dash separator and a block with two code fences. |
| `tools/qemer-demo-ingest/README.md` | Exact local build, server, staging, and manually authorized GitHub Release commands. |

## Task 1: Create the isolated tool and strict snapshot parser

**Files:**

- Create: `tools/qemer-demo-ingest/Cargo.toml`
- Create: `tools/qemer-demo-ingest/src/lib.rs`
- Create: `tools/qemer-demo-ingest/src/error.rs`
- Create: `tools/qemer-demo-ingest/src/snapshot.rs`
- Create: `tools/qemer-demo-ingest/tests/fixtures/snapshot.txt`

**Interfaces:**

- Consumes: UTF-8 Context7 export text that uses a line of exactly 32 dashes as its record delimiter.
- Produces: `snapshot::ParsedCorpus { library, version, snippets }`, `snapshot::SnippetInput`, and `snapshot::TextUnit` for later embedding.
- Later tasks call: `parse_snapshot(library: &str, version: &str, text: &str) -> Result<ParsedCorpus, IngestError>` and `ParsedCorpus::text_units() -> Vec<TextUnit>`.

- [ ] **Step 1: Create the standalone nested workspace manifest**

Create `tools/qemer-demo-ingest/Cargo.toml`. The empty `[workspace]` makes this package its own workspace, so it does not become a fourth member of the root Qemer workspace.

```toml
[workspace]

[package]
name = "qemer-demo-ingest"
version = "0.1.0"
edition = "2024"
rust-version = "1.91"
publish = false

[dependencies]
arrow-array = "=58.4.0"
arrow-schema = "=58.4.0"
clap = { version = "4.5", features = ["derive"] }
parquet = { version = "=58.4.0", features = ["arrow"] }
reqwest = { version = "0.13.4", features = ["json"] }
serde = { version = "1.0.229", features = ["derive"] }
serde_json = "1.0.151"
sha2 = "0.10"
tar = "0.4"
tempfile = "3"
thiserror = "2.0.20"
tokio = { version = "1.53.1", features = ["full"] }
zstd = "0.13"
```

- [ ] **Step 2: Add parser fixtures and failing parser tests**

Create `tests/fixtures/snapshot.txt` with two blocks. The second must contain two code fences so the test protects the observed multi-fence input shape.

````text
### First example

Source: https://example.test/first

First prose.

```python
one = 1
```

--------------------------------

### Second example

Source: https://example.test/second

Second prose.

```python
two = 2
```

```python
three = 3
```
````

Create `src/snapshot.rs` with only the types and tests below initially. Use `include_str!("../tests/fixtures/snapshot.txt")` from the module or relocate the fixture under `src/fixtures/` if needed for a stable relative path.

```rust
#[test]
fn parses_title_source_prose_and_every_code_fence() {
    let parsed = parse_snapshot("numpy", "2026-08-24", SAMPLE).unwrap();
    assert_eq!(parsed.snippets.len(), 2);
    assert_eq!(parsed.snippets[1].title, "Second example");
    assert_eq!(parsed.snippets[1].source_url, "https://example.test/second");
    assert_eq!(parsed.snippets[1].description, "Second prose.");
    assert_eq!(parsed.snippets[1].code.as_deref(), Some("two = 2\n\nthree = 3"));
}

#[test]
fn refuses_input_without_the_exact_separator() {
    let error = parse_snapshot("numpy", "2026-08-24", "### Only\n\nSource: https://x\n").unwrap_err();
    assert!(matches!(error, IngestError::MissingSeparator));
}

#[test]
fn refuses_a_block_without_a_source_url() {
    let input = "### First\n\nprose\n\n--------------------------------\n\n### Second\n\nSource: https://x\n";
    assert!(matches!(parse_snapshot("numpy", "2026-08-24", input), Err(IngestError::MissingSource { block: 1 })));
}
```

- [ ] **Step 3: Run the parser tests to verify they fail**

Run: `cargo test --manifest-path tools/qemer-demo-ingest/Cargo.toml snapshot`

Expected: FAIL because the package, `parse_snapshot`, and `IngestError` do not exist yet.

- [ ] **Step 4: Define the input model and errors**

Create `src/error.rs` and the public types in `src/snapshot.rs`. Keep errors precise enough to name the broken block and avoid embedding malformed documents.

```rust
#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("the snapshot contains no exact 32-dash separator")]
    MissingSeparator,
    #[error("block {block} has no `### ` title")]
    MissingTitle { block: usize },
    #[error("block {block} has no `Source:` URL")]
    MissingSource { block: usize },
    #[error("block {block} has an unterminated code fence")]
    UnterminatedFence { block: usize },
    #[error("block {block} has neither prose nor code")]
    EmptyBlock { block: usize },
    #[error("output directory {path} must not already exist")]
    OutputAlreadyExists { path: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("embedding request failed: {0}")]
    Embed(String),
    #[error("Parquet output failed: {0}")]
    Parquet(String),
    #[error("archive output failed: {0}")]
    Archive(String),
    #[error("manifest output failed: {0}")]
    Manifest(String),
}

#[derive(Debug, Clone)]
pub struct SnippetInput {
    pub snippet_id: String,
    pub title: String,
    pub source_url: String,
    pub description: String,
    pub code: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TextUnit {
    pub snippet_id: String,
    pub kind: &'static str,
    pub title: String,
    pub source_url: String,
    pub text: String,
}
```

- [ ] **Step 5: Implement the strict parser**

Use one delimiter constant and require at least one occurrence. Trim each split segment so the newline that follows a delimiter does not invalidate a valid heading. Parse the first non-empty line as `### <title>`, find the single `Source: ` line, and feed every subsequent non-source line through a fence state machine. Fence marker lines are not included in `code`; collected fence contents are joined by `"\n\n"`.

```rust
pub const SEPARATOR: &str = "--------------------------------";

pub fn parse_snapshot(library: &str, version: &str, text: &str) -> Result<ParsedCorpus, IngestError> {
    let blocks: Vec<String> = text.lines().collect::<Vec<_>>().split(|line| *line == SEPARATOR).map(|lines| lines.join("\n")).collect();
    if blocks.len() < 2 {
        return Err(IngestError::MissingSeparator);
    }
    let snippets = blocks.into_iter().enumerate().filter_map(|(index, block)| {
        (!block.trim().is_empty()).then(|| parse_block(library, version, index + 1, &block))
    }).collect::<Result<Vec<_>, _>>()?;
    Ok(ParsedCorpus { library: library.into(), version: version.into(), snippets })
}

impl ParsedCorpus {
    pub fn text_units(&self) -> Vec<TextUnit> {
        self.snippets.iter().flat_map(|snippet| {
            let prose = (!snippet.description.is_empty()).then(|| TextUnit {
                snippet_id: snippet.snippet_id.clone(), kind: "prose", title: snippet.title.clone(), source_url: snippet.source_url.clone(), text: snippet.description.clone(),
            });
            let code = snippet.code.as_ref().map(|text| TextUnit {
                snippet_id: snippet.snippet_id.clone(), kind: "code", title: snippet.title.clone(), source_url: snippet.source_url.clone(), text: text.clone(),
            });
            prose.into_iter().chain(code)
        }).collect()
    }
}
```

Make `parse_block` private and give snippet IDs the deterministic form `{library}-{version}-{ordinal:06}`. Do not use title text or source URLs in the ID.

- [ ] **Step 6: Export the modules and run the parser tests**

Create `src/lib.rs` with only the modules implemented in this task:

```rust
pub mod error;
pub mod snapshot;

pub use error::IngestError;
```

Run: `cargo test --manifest-path tools/qemer-demo-ingest/Cargo.toml snapshot`

Expected: PASS. The tests prove delimiter enforcement, metadata extraction, and non-lossy handling of multiple fenced examples.

- [ ] **Step 7: Review checkpoint**

Review the parser diff and test output. Do not commit unless the user separately asks.

## Task 2: Add the embedding client and vector-width gate

**Files:**

- Create: `tools/qemer-demo-ingest/src/embed.rs`
- Modify: `tools/qemer-demo-ingest/src/error.rs`
- Modify: `tools/qemer-demo-ingest/src/lib.rs`
- Test: `tools/qemer-demo-ingest/src/embed.rs`

**Interfaces:**

- Consumes: `snapshot::TextUnit`, an embedding base URL, a supplied model name, and a supplied vector dimension.
- Produces: `embed::EmbeddedUnit { unit: TextUnit, vector: Vec<f32> }`.
- Later tasks call: `EmbeddingClient::new(base_url, model, dimension)` and `EmbeddingClient::embed_all(Vec<TextUnit>) -> Result<Vec<EmbeddedUnit>, IngestError>`.

- [ ] **Step 1: Write failing response and request-shape tests**

Use a fake OpenAI-shaped JSON body. Test the pure response parser separately from HTTP so width failures do not require a server.

```rust
#[test]
fn parses_the_first_embedding_at_the_configured_width() {
    let body = br#"{"data":[{"embedding":[1.0,2.0,3.0]}]}"#;
    assert_eq!(parse_embedding(body, 3).unwrap(), vec![1.0, 2.0, 3.0]);
}

#[test]
fn rejects_a_response_with_the_wrong_width() {
    let body = br#"{"data":[{"embedding":[1.0,2.0]}]}"#;
    let error = parse_embedding(body, 3).unwrap_err();
    assert!(error.to_string().contains("expected 3 dimensions"));
}

#[test]
fn request_body_carries_only_the_text_and_configured_model() {
    let body = embedding_request("some code", "nomic-embed-text-v1.5");
    assert_eq!(body["input"], "some code");
    assert_eq!(body["model"], "nomic-embed-text-v1.5");
}
```

- [ ] **Step 2: Run embedding tests to verify they fail**

Run: `cargo test --manifest-path tools/qemer-demo-ingest/Cargo.toml embed`

Expected: FAIL because `embed.rs`, `parse_embedding`, and `embedding_request` do not exist.

- [ ] **Step 3: Implement a single-request OpenAI-compatible client**

Do not assume bulk `input` support from the embedding server. Reuse the known-good request shape already used by Qemer core, one text unit per request, and preserve unit order. The corpus has only 1,109 snippets, so bounded sequential requests are sufficient for this demo and avoid an unverified server-specific batching protocol.

```rust
#[derive(Debug, Clone)]
pub struct EmbeddingClient {
    http: reqwest::Client,
    base_url: String,
    model: String,
    dimension: usize,
}

pub fn embedding_request(text: &str, model: &str) -> serde_json::Value {
    serde_json::json!({ "input": text, "model": model })
}

pub fn parse_embedding(body: &[u8], expected_dim: usize) -> Result<Vec<f32>, IngestError> {
    #[derive(serde::Deserialize)]
    struct Response { data: Vec<Datum> }
    #[derive(serde::Deserialize)]
    struct Datum { embedding: Vec<f32> }

    let response: Response = serde_json::from_slice(body).map_err(|e| IngestError::Embed(format!("invalid embeddings response: {e}")))?;
    let vector = response.data.into_iter().next().ok_or_else(|| IngestError::Embed("response contained no embeddings".into()))?.embedding;
    if vector.len() != expected_dim {
        return Err(IngestError::Embed(format!("expected {expected_dim} dimensions, received {}", vector.len())));
    }
    Ok(vector)
}
```

`embed_one` must POST to `format!("{}/v1/embeddings", base_url.trim_end_matches('/'))`, call `error_for_status()`, and put both the endpoint and the underlying error in `IngestError::Embed`. `embed_all` must stop at the first failure; never write a partial corpus with mixed embedding results.

- [ ] **Step 4: Export the completed embedding module**

Update `src/lib.rs`:

```rust
pub mod embed;
pub mod error;
pub mod snapshot;

pub use error::IngestError;
```

- [ ] **Step 5: Add one HTTP-stub integration test**

In `embed.rs`’s test module, bind a Tokio `TcpListener` to `127.0.0.1:0`, accept one request, read through the headers and declared `Content-Length`, assert that the JSON body contains the expected `input` and `model`, then return a response with an exact dynamic content length:

```rust
let body = r#"{"data":[{"embedding":[1.0,2.0,3.0]}]}"#;
let response = format!(
    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
    body.len(),
);
```

Invoke `EmbeddingClient::embed_one` against that listener and assert the returned vector. This verifies the wire shape without starting the supplied GGUF or contacting the network.

- [ ] **Step 6: Run all embedding tests**

Run: `cargo test --manifest-path tools/qemer-demo-ingest/Cargo.toml embed`

Expected: PASS, including the local HTTP stub test and the wrong-dimension rejection.

- [ ] **Step 7: Review checkpoint**

Confirm the tool only calls the configured endpoint and contains no process-spawning or model-download code. Do not commit unless the user separately asks.

## Task 3: Write contract-compatible Parquet, archives, checksums, and manifest

**Files:**

- Create: `tools/qemer-demo-ingest/src/package.rs`
- Modify: `tools/qemer-demo-ingest/src/error.rs`
- Modify: `tools/qemer-demo-ingest/src/lib.rs`
- Test: `tools/qemer-demo-ingest/src/package.rs`

**Interfaces:**

- Consumes: `Vec<embed::EmbeddedUnit>`, corpus identity, and a public release-asset base URL.
- Produces: `<library>-<version>.tar.zst`, a `manifest.json`, and `package::ManifestEntry`.
- Later tasks call: `stage_corpus(output_dir, identity, rows) -> Result<ManifestEntry, IngestError>` and `write_manifest(output_dir, entries) -> Result<(), IngestError>`.

- [ ] **Step 1: Write failing artifact tests**

Create a two-row fixture with one prose vector and one code vector of dimension three. Test that Parquet retains the exact required column names, that the vector is a fixed-size list with the supplied dimension, and that the compressed tar contains exactly `corpus.parquet` at its root.

```rust
#[test]
fn parquet_uses_the_qemer_row_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corpus.parquet");
    write_parquet(&path, &rows(), 3).unwrap();
    let reader = parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(path).unwrap()).unwrap().build().unwrap();
    let schema = reader.schema();
    assert_eq!(schema.field(0).name(), "snippet_id");
    assert_eq!(schema.field(1).name(), "kind");
    assert_eq!(schema.field(5).name(), "vector");
    assert!(matches!(schema.field(5).data_type(), arrow_schema::DataType::FixedSizeList(_, 3)));
}

#[test]
fn archive_places_parquet_at_the_installers_expected_path() {
    let dir = tempfile::tempdir().unwrap();
    let parquet = dir.path().join("corpus.parquet");
    write_parquet(&parquet, &rows(), 3).unwrap();
    let archive = dir.path().join("numpy-2026-08-24.tar.zst");
    write_archive(&parquet, &archive).unwrap();
    let decoder = zstd::stream::read::Decoder::new(std::fs::File::open(archive).unwrap()).unwrap();
    let names = tar::Archive::new(decoder).entries().unwrap().map(|entry| entry.unwrap().path().unwrap().into_owned()).collect::<Vec<_>>();
    assert_eq!(names, vec![std::path::PathBuf::from("corpus.parquet")]);
}
```

- [ ] **Step 2: Run packaging tests to verify they fail**

Run: `cargo test --manifest-path tools/qemer-demo-ingest/Cargo.toml package`

Expected: FAIL because the artifact-writing functions do not exist.

- [ ] **Step 3: Implement the exact Arrow and Parquet layout**

Use Arrow 58.4 directly because this independent tool does not depend on LanceDB. Pinning Arrow and Parquet to exactly `58.4.0` prevents the incompatible-version error that Qemer avoids through LanceDB’s re-exports.

```rust
pub fn corpus_schema(dimension: i32) -> arrow_schema::SchemaRef {
    use arrow_schema::{DataType, Field, Schema};
    use std::sync::Arc;
    Arc::new(Schema::new(vec![
        Field::new("snippet_id", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("source_url", DataType::Utf8, true),
        Field::new("text", DataType::Utf8, false),
        Field::new("vector", DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), dimension), false),
    ]))
}

pub fn write_archive(parquet: &Path, archive: &Path) -> Result<(), IngestError> {
    let encoder = zstd::stream::write::Encoder::new(std::fs::File::create(archive)?, 19).map_err(|e| IngestError::Archive(e.to_string()))?;
    let mut tar = tar::Builder::new(encoder);
    tar.append_path_with_name(parquet, "corpus.parquet").map_err(|e| IngestError::Archive(e.to_string()))?;
    let encoder = tar.into_inner().map_err(|e| IngestError::Archive(e.to_string()))?;
    encoder.finish().map_err(|e| IngestError::Archive(e.to_string()))?;
    Ok(())
}
```

Build the fixed-size vector column with `FixedSizeListBuilder<Float32Builder>`. Before appending, reject any `EmbeddedUnit` whose vector length differs from `dimension`; an output writer must never be the first place a bad vector is discovered. Use `parquet::arrow::ArrowWriter`, write one `RecordBatch`, then call `close()`.

- [ ] **Step 4: Implement archive metadata and manifest generation**

Define manifest structs with `serde::Serialize` and names matching Qemer core’s `CorpusRef` exactly.

```rust
#[derive(Debug, serde::Serialize)]
pub struct Manifest { pub corpora: Vec<ManifestEntry> }

#[derive(Debug, serde::Serialize)]
pub struct ManifestEntry {
    pub library: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub bytes: u64,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub snippet_count: usize,
}

#[derive(Debug, Clone)]
pub struct CorpusIdentity {
    pub library: String,
    pub version: String,
    pub embedding_model: String,
    pub embedding_dim: usize,
    pub asset_base_url: String,
}

pub fn sha256_file(path: &Path) -> Result<String, IngestError> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
```

`stage_corpus` writes `corpus.parquet` inside a `tempfile::TempDir`, names the final archive `{library}-{version}.tar.zst` in `output_dir`, calculates both checksum and file length after compression, and forms its URL as `{asset_base_url_without_trailing_slash}/{archive_name}`. The temporary Parquet file is dropped after the archive is complete, so the staging directory ends with only release assets. `write_manifest` serializes `{ "corpora": [...] }` in pretty JSON to `output_dir/manifest.json` only after both entries are ready.

- [ ] **Step 5: Export the completed packaging module**

Update `src/lib.rs`:

```rust
pub mod embed;
pub mod error;
pub mod package;
pub mod snapshot;

pub use error::IngestError;
```

- [ ] **Step 6: Add manifest tests and run all package tests**

Add a test that stages the three-dimension fixture with base `https://github.com/example/qemer-corpora/releases/download/demo-2026-08-25`, reads `manifest.json`, and asserts the URL, checksum length of 64, byte count greater than zero, model name, dimension, and snippet count.

Run: `cargo test --manifest-path tools/qemer-demo-ingest/Cargo.toml package`

Expected: PASS. The archive layout, Parquet schema, and manifest shape now match the Qemer consumer contract.

- [ ] **Step 7: Review checkpoint**

Inspect the archive test against `qemer-core/src/corpus.rs`: it must contain `corpus.parquet` at archive root because `Cache::install` unpacks and opens exactly that path. Do not commit unless the user separately asks.

## Task 4: Add the explicit CLI, documentation, and local staging workflow

**Files:**

- Create: `tools/qemer-demo-ingest/src/main.rs`
- Create: `tools/qemer-demo-ingest/README.md`
- Test: `tools/qemer-demo-ingest/src/main.rs`

**Interfaces:**

- Consumes: two snapshot paths, explicit corpus identities, explicit endpoint/model/dimension, an empty output directory, and a GitHub Release asset base URL.
- Produces: `manifest.json`, `numpy-2026-08-24.tar.zst`, and `pytorch-2026-08-24.tar.zst` in the output directory.
- External boundary: it stages local files only; GitHub publication is manual and is not invoked by the tool.

- [ ] **Step 1: Write failing argument-validation tests**

Keep CLI orchestration testable without a terminal. Extract `validate_output_dir` and `fixed_corpora` from `main`.

```rust
#[test]
fn output_directory_must_not_already_exist() {
    let dir = tempfile::tempdir().unwrap();
    let error = validate_output_dir(dir.path()).unwrap_err();
    assert!(error.to_string().contains("must not already exist"));
}

#[test]
fn corpus_arguments_preserve_the_dated_snapshot_versions() {
    let corpora = fixed_corpora("/tmp/numpy.txt".into(), "/tmp/pytorch.txt".into());
    assert_eq!(corpora[0].library, "numpy");
    assert_eq!(corpora[0].version, "2026-08-24");
    assert_eq!(corpora[1].library, "pytorch");
    assert_eq!(corpora[1].version, "2026-08-24");
}
```

- [ ] **Step 2: Run the CLI tests to verify they fail**

Run: `cargo test --manifest-path tools/qemer-demo-ingest/Cargo.toml main`

Expected: FAIL because `main.rs`, `fixed_corpora`, and `validate_output_dir` do not exist.

- [ ] **Step 3: Implement explicit arguments and orchestration**

Make all environment-specific inputs mandatory. Do not default to the user’s home directory, a release owner, an endpoint, a model name, or a dimension.

```rust
#[derive(Debug, clap::Parser)]
#[command(name = "qemer-demo-ingest", about = "Build the NumPy and PyTorch demo corpora from local snapshots")]
struct Args {
    #[arg(long)] numpy: std::path::PathBuf,
    #[arg(long)] pytorch: std::path::PathBuf,
    #[arg(long)] output: std::path::PathBuf,
    #[arg(long)] asset_base_url: String,
    #[arg(long)] embedding_url: String,
    #[arg(long)] embedding_model: String,
    #[arg(long)] embedding_dim: usize,
}

const NUMPY_VERSION: &str = "2026-08-24";
const PYTORCH_VERSION: &str = "2026-08-24";

#[derive(Debug, Clone)]
struct InputCorpus {
    library: &'static str,
    version: &'static str,
    snapshot: std::path::PathBuf,
}

fn fixed_corpora(numpy: std::path::PathBuf, pytorch: std::path::PathBuf) -> [InputCorpus; 2] {
    [
        InputCorpus { library: "numpy", version: NUMPY_VERSION, snapshot: numpy },
        InputCorpus { library: "pytorch", version: PYTORCH_VERSION, snapshot: pytorch },
    ]
}

fn validate_output_dir(path: &std::path::Path) -> Result<(), qemer_demo_ingest::IngestError> {
    if path.exists() {
        return Err(qemer_demo_ingest::IngestError::OutputAlreadyExists { path: path.display().to_string() });
    }
    Ok(())
}
```

`main` must call `validate_output_dir` before reading either snapshot or making any HTTP request. It must create the output directory only after validation. For each fixed identity, read the supplied snapshot, call `parse_snapshot`, turn the parsed snippets into text units, embed every unit, stage one archive, collect its `ManifestEntry`, then write the final manifest after both archive operations succeed. If any step fails, return an error and leave an incomplete output directory for diagnosis; never overwrite an existing staging directory.

- [ ] **Step 4: Add the operator README**

Write `tools/qemer-demo-ingest/README.md` with the following exact flow. Keep all prose paragraphs on one line.

```bash
# Terminal 1: the user starts the model server; the tool never does this.
/home/eyob-g/.local/bin/llama-server --model /home/eyob-g/Downloads/nomic-embed-text-v1.5.f16.gguf --embedding --port 8080 --no-webui

# Terminal 2: build local, release-ready assets. The output directory must not exist.
cargo run --manifest-path tools/qemer-demo-ingest/Cargo.toml -- \
  --numpy /home/eyob-g/Downloads/26-08-24-numpy-llms.txt \
  --pytorch /home/eyob-g/Downloads/26-08-24-pytorch-llms.txt \
  --output /tmp/qemer-demo-corpora \
  --asset-base-url https://github.com/OWNER/qemer-corpora/releases/download/demo-2026-08-25 \
  --embedding-url http://127.0.0.1:8080 \
  --embedding-model nomic-embed-text-v1.5 \
  --embedding-dim 768
```

Then document the staged output files and a separately labeled, manually run GitHub CLI command:

```bash
gh release create demo-2026-08-25 \
  /tmp/qemer-demo-corpora/manifest.json \
  /tmp/qemer-demo-corpora/numpy-2026-08-24.tar.zst \
  /tmp/qemer-demo-corpora/pytorch-2026-08-24.tar.zst \
  --repo OWNER/qemer-corpora \
  --title "Qemer demo corpora 2026-08-25"
```

Immediately before that command, state that it creates publicly downloadable release assets, and that correcting their names, URLs, or contents later requires replacing or deleting the published release. This is the publication one-way door; do not place this command in code or automated tests.

- [ ] **Step 5: Run the complete tool test suite and static checks**

Run: `cargo fmt --manifest-path tools/qemer-demo-ingest/Cargo.toml -- --check`

Expected: PASS.

Run: `cargo clippy --manifest-path tools/qemer-demo-ingest/Cargo.toml --all-targets -- -D warnings`

Expected: PASS with no warnings.

Run: `cargo test --manifest-path tools/qemer-demo-ingest/Cargo.toml`

Expected: PASS, including parser, HTTP-stub, Parquet, archive, manifest, and CLI-validation tests.

- [ ] **Step 6: Stage the real local corpora without publishing**

Start the server manually using the README command, then run the tool with a fresh output directory beneath `/tmp`. Confirm the resulting directory contains exactly:

```text
manifest.json
numpy-2026-08-24.tar.zst
pytorch-2026-08-24.tar.zst
```

Inspect `manifest.json` with `jq .` and record the snippet counts, archive byte sizes, model name, vector dimension, and checksums. Do not run the GitHub CLI command in this task.

- [ ] **Step 7: Perform the Qemer consumer smoke test after explicit publication approval**

Only after the user has explicitly authorized publishing and the GitHub Release exists, point a complete temporary Qemer config at the release `manifest.json` URL. Install both corpora with the currently built binary name:

```bash
cargo run -p qemer-tui -- install numpy@2026-08-24
cargo run -p qemer-tui -- install pytorch@2026-08-24
```

The install must verify the manifest checksums, unpack each archive, build its LanceDB table, and print an installed path. Then run `cargo run -p qemer-tui -- list` and confirm both dated corpora appear. This checks the producer/consumer boundary without adding any ingestion knowledge to Qemer runtime code.

- [ ] **Step 8: Review checkpoint**

Review the README’s public-release warning and the actual generated manifest before requesting publication authority. Do not commit unless the user separately asks.

## Plan Coverage Review

- Local-only source inputs: Task 4 requires explicit local paths; no task fetches documentation.
- Demo-only separator exception: Task 1 enforces exactly 32 dashes and tests failure without it.
- Multiple code fences: Task 1 joins all fence contents and tests the result.
- User-run model runtime: Task 2 is HTTP-only and Task 4 documents a manual `llama-server` command.
- Embedding identity safety: Task 2 rejects wrong widths and Task 4 requires explicit model and dimension; Task 3 stamps them in the manifest.
- Qemer data contract: Task 3 writes the specified Parquet schema, archive root, checksum, byte count, and manifest fields.
- GitHub Releases: Task 4 prepares valid asset URLs and documents, but does not automate, the externally visible release command.
- Validation: parser, HTTP protocol, Parquet schema, archive layout, manifest contents, CLI safety, real staging, and post-publication Qemer installation each have an explicit verification step.

## Placeholder and Consistency Review

The plan defines every referenced module, public type, function, archive filename, version, and command. It contains no deferred implementation steps, automatic publishing, model-runtime launch, or unspecified data format.
