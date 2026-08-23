# Qemer

An offline coding assistant grounded in technical documentation. It downloads prebuilt, pre-embedded documentation corpora, searches them locally, and answers questions with a local LLM using the retrieved snippets as grounding.

Read `docs/decisions.md` before designing anything. It records what has been settled and, more importantly, what has not — several load-bearing questions are open, and the failure mode for this project is an agent inventing a reasonable answer to one of them instead of asking.

## Architecture rules

Three crates, dependencies pointing one direction only:

- `qemer-core` — retrieval. Corpus discovery, download, verification, query embedding, vector search.
- `qemer-answer` — grounding and generation. Depends on `qemer-core` for the `Snippet` type only.
- `qemer-tui` — ratatui binary. Depends on both.

**`qemer-core` must never depend on `qemer-answer`, and must never reference generation.** The reason is a consumer that does not exist yet: a `qemer-mcp` server exposing the same corpora to coding agents that bring their own model. It links `qemer-core` alone. Any generation concern that leaks into core breaks that.

**`qemer-answer` never retrieves.** Callers hand it snippets. It has no LanceDB dependency and no knowledge of how snippets were found.

## Boundaries

**Qemer never installs, downloads, or launches a model runtime.** It talks HTTP to a `llama-server` the user is already running. When it cannot reach one, it says so and states what to start. GPU toolchains stay outside this binary.

**Corpora are built elsewhere.** `qemer-ingest` is a separate repository that fetches documentation, parses it, embeds it, and publishes tarballs. This repository only consumes them. The contract is the manifest plus tarball layout in `docs/decisions.md` — do not write code, comments, or docs here that assume anything further about how ingestion works.

**Embedding model mismatches must fail loudly.** Every corpus carries the model name and dimension it was built with. Querying a corpus with a different model produces plausible-looking nonsense rather than an error, so the check is not optional and must run before any search.

## Build

`cargo build` requires `protoc` on PATH — `lance-encoding` compiles protobuf definitions in its build script. On Debian/Ubuntu: `apt install protobuf-compiler`. The first build compiles datafusion and takes several minutes.

## Testing

Prompt assembly in `qemer-answer/src/prompt.rs` is pure functions over data, and it is where the edge cases live: a single code block larger than the whole budget, retrieval returning nothing, truncation landing mid-identifier. Test those directly. Do not reach for a terminal harness to test logic that never touches a terminal.
