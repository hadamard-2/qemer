# Qemer

An offline coding assistant grounded in technical documentation — an open-source take on Context7, running entirely against a local model runtime.

Qemer downloads prebuilt, pre-embedded documentation corpora, searches them locally with vector similarity, and answers questions using a local LLM with the retrieved snippets as grounding. Nothing leaves the machine at query time.

Status: **early scaffold.** The crate boundaries and public types exist; the implementations are `todo!()`.

## How it fits together

Three crates, with dependencies pointing one direction only.

| Crate | Responsibility | Depends on |
| --- | --- | --- |
| `qemer-core` | Corpus discovery, download, verification; query embedding; vector search. Knows nothing about generation. | — |
| `qemer-answer` | Prompt assembly, context budgeting, streamed generation. Never retrieves — callers hand it snippets. | `qemer-core` (types only) |
| `qemer-tui` | Ratatui interface. Orchestrates: search, show sources, then stream the answer. | both |

The split exists so that retrieval has more than one consumer. A future `qemer-mcp` server, exposing the same corpora to coding agents that bring their own model, links `qemer-core` alone and never compiles the generation path.

## Requirements

Qemer does not install or launch a model runtime. It talks HTTP to a `llama-server` you are already running, and tells you what to start if it cannot reach one. You need two models loaded: an embedding model and a chat model.

The embedding model is not a free choice — every published corpus is embedded with a specific model at a specific dimensionality, and querying it with anything else produces plausible nonsense rather than an error. So the model name and dimension are stamped into each corpus manifest and checked before any search runs; a mismatch fails loudly.

Current corpus embedding model: **nomic-embed-text-v1.5**, 768 dimensions.

## Corpora

Corpora are built and published by **`qemer-ingest`**, a separate repository. This repository only consumes them, and the contract between the two is a manifest plus a tarball layout:

```
Manifest → [ { library, version, url, sha256, bytes,
               embedding_model, embedding_dim, snippet_count } ]
```

Nothing in this codebase should assume anything further about how ingestion works.

## Design decisions

Settled decisions, the reasoning behind them, and — importantly — the questions still open are in [`docs/decisions.md`](docs/decisions.md). Several load-bearing choices are deliberately unresolved; check there before assuming one.

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
