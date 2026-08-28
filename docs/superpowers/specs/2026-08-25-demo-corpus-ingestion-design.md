# Demo corpus ingestion: NumPy and PyTorch

**Status:** approved design, awaiting written-spec review.

## Purpose and scope

Produce two demonstration corpora that Qemer can install and query: NumPy and PyTorch. The input documentation is already present locally as dated Context7 exports, so this work does not download source documentation or a model. It produces the existing manifest-plus-tarball contract that `qemer-core` consumes without changing the runtime crates.

This is intentionally a narrow, demo-only producer. It is not the general `qemer-ingest` repository described in `docs/decisions.md`, and it does not establish a general ingestion architecture. A future standalone ingester must return to the settled production decisions unless those decisions are explicitly revisited.

## Inputs

- `/home/eyob-g/Downloads/26-08-24-numpy-llms.txt`
- `/home/eyob-g/Downloads/26-08-24-pytorch-llms.txt`
- An already-running embedding server that serves `/home/eyob-g/Downloads/nomic-embed-text-v1.5.f16.gguf` through an OpenAI-compatible embeddings endpoint.

The GGUF is a model artifact, not a service. The demo tool receives an embedding-server base URL and performs HTTP requests; it never downloads, installs, or launches a model runtime.

## Corpus identity

The source files are rolling documentation exports, not verified upstream library releases. Their dated filenames therefore define snapshot versions:

- `numpy@2026-08-24`
- `pytorch@2026-08-24`

Each manifest entry stamps the supplied embedding model name and vector dimension. Qemer will reject a query if its configured embedding client does not exactly match this stamp.

## Parsing

The source exports visibly contain `###` headings, `Source:` lines, prose, fenced code blocks, and 32-dash separators. For this demonstration the parser splits only on a line that is exactly 32 dashes. This deliberately overrides the production ingestion decision to split on headings because the two supplied local snapshots make the separator an explicit input guarantee.

The parser validates the expected format before embedding: it must find separators and every emitted block must have a title and source URL. A malformed input fails the build rather than emitting a corpus with fused or untraceable snippets. Within each block, prose becomes `text` for a `prose` row and fenced code becomes `text` for an optional `code` row; both rows share a snippet ID, title, and source URL. A block may contain multiple fenced examples, as the supplied snapshots do; their fence contents are joined with two newlines into the one code row allowed by the corpus contract.

## Tool boundary and output

A repository-local, explicitly named demo tool under `tools/` reads the two absolute input paths and writes a release staging directory. It has no import path or runtime dependency from `qemer-core`, `qemer-answer`, or `qemer-tui`.

For each corpus it emits a Parquet file with the established row schema, builds the tar.zst archive expected by Qemer, and writes a `manifest.json` containing the two corpus entries. Generated Parquet, tarballs, and manifests are staging artifacts and are not committed to Git.

## Publication

GitHub Releases is the initial distribution channel. The release receives `manifest.json` and one `.tar.zst` archive per corpus. Qemer is configured with the public URL of that manifest. Publication is separate from generation: the tool stages assets locally, and a release is created or updated only with explicit user authorization.

Cloudflare R2 remains a compatible future host because the artifacts and manifest URLs are plain HTTP resources; migrating hosts changes the manifest URLs, not the archive format or Qemer runtime code.

## Verification

The demo tool is tested against small local fixtures for separator validation, metadata extraction, and emitted rows. An end-to-end test uses a stub embeddings endpoint to verify model stamping, output layout, and manifest shape without loading the GGUF. A manual release-ready check opens the staged archive through Qemer's existing install path, then queries both installed corpora against a real user-run embedding server and completion server.

## Explicit non-goals

- No live documentation downloads or scraping.
- No automatic model-server startup or model download.
- No corpus browsing, version-selection policy, cache eviction, or TUI install flow.
- No generalized multi-source ingestion framework.
- No external publication without a separate explicit request.
