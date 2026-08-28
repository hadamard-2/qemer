# Qemer

An offline coding assistant grounded in technical documentation. Qemer searches installed corpora locally and answers questions using retrieved snippets as grounding.

## Requirements

Qemer imports prebuilt corpora from an explicit manifest. It does not crawl, scrape, or embed a corpus, and it does not install, download, or launch a model runtime.

Provide the two `llama-server` endpoints Qemer uses yourself: one for embeddings and one for completions. `qemer config` records their URLs, models, and the completion context settings Qemer needs; both values can point to one server when it serves both. Qemer reports when an endpoint cannot be reached and tells you what to start.

The embedding model is not a free choice. Every corpus records the model name and dimensionality used to build it; Qemer checks both before search, and rejects a mismatch rather than returning plausible but invalid results.

Current corpus embedding model: **nomic-embed-text-v1.5**, 768 dimensions.

## Install a local corpus

Place the manifest beside its `.tar.zst` artifact when the manifest's `url` is relative, then run:

```sh
qemer config
qemer available --manifest /absolute/path/to/manifest.json
qemer install numpy@2.3.0 --manifest /absolute/path/to/manifest.json
qemer list
qemer
```

The manifest lists each corpus's library, version, artifact URL, advertised byte count, SHA-256, embedding model and dimension, and snippet count. Before installing, Qemer verifies the advertised byte count and SHA-256. A manifest can be a local path or an HTTPS URL; a relative artifact URL resolves beside the local manifest or relative to the HTTPS manifest.

## How it fits together

Three crates, with dependencies pointing one direction only.

| Crate | Responsibility | Depends on |
| --- | --- | --- |
| `qemer-core` | Corpus discovery, artifact download, verification; query embedding; vector search. Knows nothing about generation. | — |
| `qemer-answer` | Prompt assembly, context budgeting, streamed generation. Never retrieves — callers hand it snippets. | `qemer-core` (types only) |
| `qemer-tui` | Ratatui interface. Orchestrates: search, show sources, then stream the answer. | both |

The split exists so that retrieval has more than one consumer. A future `qemer-mcp` server, exposing the same corpora to coding agents that bring their own model, links `qemer-core` alone and never compiles the generation path.

## Corpus contract

The manifest and `.tar.zst` artifact are the complete consumer contract:

```
Manifest → [ { library, version, url, sha256, bytes,
               embedding_model, embedding_dim, snippet_count } ]
```

The artifact contains `corpus.parquet`, with one row for each prose-or-code unit. See [`docs/decisions.md`](docs/decisions.md) for the complete artifact layout and the settled design decisions.

## Development

Building requires `protoc` on PATH — `lance-encoding` compiles protobuf definitions in its build script. On Debian/Ubuntu:

```sh
sudo apt install protobuf-compiler
```

Then:

```sh
cargo build
cargo test
```

The first build compiles datafusion and takes several minutes.
