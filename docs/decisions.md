# Design decisions

Settled in discussion rather than in a spec document. Recorded here because the reasoning is not recoverable from the code, and because knowing which questions are *still open* matters as much as knowing the answers.

## Settled

**Library-scoped queries, not open search.** You choose a library, then ask; retrieval never crosses library boundaries. Routing a question across libraries is a separate hard problem, and folding it in means a wrong answer could be a retrieval failure or a routing failure with no way to tell which. Context7 works the same way — an agent resolves a library ID first, then queries.

**Snippet-as-row, not fixed-size chunks.** Context7's curated markdown is already segmented into titled blocks carrying a source URL, prose, and usually one code block. Re-chunking by token window discards exactly the curation that makes the corpus worth using.

**Split on `^### ` headings, not on dash separators.** Blocks are usually delimited by a run of dashes, but not reliably — observed output has consecutive blocks with no separator between them. Splitting on dashes silently fuses two snippets into one oversized row; splitting on headings gives the same result on well-formed input and degrades safely on malformed input.

**Prose and code embedded separately.** Embedding a whole block lets code tokens dominate the vector, which general-purpose embedding models handle poorly. `code` is nullable — Context7 emits description-only blocks, and they are worth keeping for conceptual queries.

**Prebuilt corpora, not local ingestion.** Qemer imports prebuilt artifacts from an explicit local or HTTPS manifest source rather than computing vectors. The one-time corpus embedding pass is the expensive part; query-time embedding is milliseconds.

**Manifest identity is `(library, version)`.** A manifest cannot list the same library and version more than once. Artifact URLs may be relative to the manifest source, but the client resolves and validates them before installation.

**HTTP to `llama-server`, no bundled runtime.** GPU toolchains are the least portable part of this stack, so they stay outside the binary. This also means the TUI and a future MCP server point at an endpoint the user runs rather than each managing a process.

**nomic-embed-text-v1.5 at 768 dimensions.** Chosen for its 8192-token context, which means snippet length never needs thinking about, and for Matryoshka support, which allows shrinking vectors later without changing models — the one dimension change that does not invalidate every published corpus. The model name and dimension are stamped into each corpus and checked before search.

**Three crates, not two.** The seam between retrieval and generation exists because retrieval has a second consumer coming. The seam between generation and the TUI exists because prompt assembly and context budgeting are the logic most worth unit-testing and the terminal is the hardest place to test from.

**Conversational answers, for now.** The generated answer is the primary content, with snippets available beneath it — rather than a three-line orientation above a source list. This is an experiment, not a conviction: the local model is small (Qwen3.5-0.8B at Q4_K_M), and small models confabulate confidently when retrieval misses. If the answers prove unreliable, fall back to **sources-first**: cap generation at roughly 80 tokens, have it point at which snippet is relevant rather than explain, and let the snippets carry the content. That fallback changes only the prompt, the token cap, and the TUI's visual weighting — no structural change.

**Corpora ship as Parquet, not as built tables.** A tarball contains raw rows plus vectors; the client builds the LanceDB table on install. Shipping a `.lance` directory would make every published corpus depend on the storage version of the Lance release that built it, and that dependency fails at open time on a user's machine, for a corpus nobody is rebuilding. Parquet keeps the contract a data contract — readable by anything, including a debugging script or a future consumer that never links LanceDB. The cost is a client-side build step at install; at a few thousand rows it is seconds.

**One LanceDB database directory per library and version.** Retrieval never crosses library boundaries, so a shared table would carry a `library` column whose only job is to be filtered on every query — a predicate that can never be absent and can never match more than one value. Separate directories make an installed corpus exactly one directory: install is build-to-temp then atomic rename, uninstall is one `remove_dir_all`, and a failed install cannot corrupt a corpus that was already fine. It also makes the embedding-model check structural rather than disciplinary — a corpus's model stamp lives beside the corpus it guards, so there is no way to check one and search another.

**Prose and code are two rows in one table, sharing a `snippet_id`.** One vector column, each row tagged `kind: prose | code`. This is chosen over two vector columns on one row because `code` is nullable: with two rows, "this snippet has no code" is a row that does not exist, rather than a null vector whose search behavior would have to be verified and could fail silently rather than loudly.

**Hybrid retrieval: BM25 and vector, fused by RRF.** Embeddings are weakest exactly where this corpus is strongest — a query for a literal identifier like `create_fts_index` embeds to something near its English paraphrase and will rank a conceptually adjacent snippet above the one containing the actual symbol. Half the corpus is code blocks full of exact identifiers, so lexical and semantic retrieval are complementary here rather than redundant. LanceDB provides both natively: `Index::FTS` is BM25, and `rerankers::rrf::RRFReranker` fuses the two result sets.

Fusion is by reciprocal rank, not by combining scores. BM25 scores are unbounded and corpus-dependent while cosine similarity is bounded, so there is no principled constant that linearly combines them and any normalization would be a tuning knob with no evidence behind it. RRF discards the scores and uses only ranks, which is why it is the right tool here — and why it would have been the wrong tool for a single ranking in a single metric.

**Merging happens in two layers, and rows collapse before lists fuse.** Each retriever runs its own search and over-fetches, each row list is collapsed to snippets by taking the **maximum** score among a snippet's rows, and only then are the two snippet-level rankings fused by RRF. The order matters and does not commute: fusing at the row level first would let a snippet whose prose *and* code rows both appear in the BM25 list contribute two rank terms from one retriever, double-counting a single retriever's opinion. Collapsing first gives each retriever exactly one vote per snippet. Max-of-scores within a list is deliberate — a snippet whose code block answers the query should rank as high as one whose prose does, not be averaged down by a mediocre sibling. No bonus for matching on both prose and code; that is a knob with no evidence behind it until retrieval quality is measurable.

**The FTS index covers `text` and `title`.** Titles are short and dense with the library's own terminology, so they match queries phrased as feature names. This accepts a known BM25 characteristic — short fields score high on length normalization — as a reasonable trade for that recall.

**No ANN index; the FTS index is required.** `full_text_search` needs an FTS index to exist, so the install-time build creates one. It is derived client-side from the same rows, so nothing in the tarball or manifest changes. The decision below is specifically about *approximate nearest neighbour* indexing. Under 10,000 vectors of 768 f32s is roughly 25 MB, and an exhaustive scan is a few million multiply-adds — well under a millisecond, and one to two orders of magnitude cheaper than the query embedding round-trip that feeds it. LanceDB's ANN index is IVF-PQ, and both halves work against this size: IVF trains centroids on noise at 10k rows, and PQ is lossy, so it would trade exact recall for an invisible latency win. **Revisit if a single corpus exceeds roughly 100k vectors, or if measured search time becomes a visible fraction of total query latency.** Neither is close.

**Over-fetch is per retriever.** Each of the two searches asks for `3k` rows, so each has enough depth to yield `k` snippets after collapsing before RRF sees it.

**Two `llama-server` base URLs, not one.** Config carries an embedding URL and a completion URL, defaulting to different ports, with nothing in the code requiring them to differ — a single multi-model server is the deployment where both keys hold the same value. Two keys support both deployments; one key supports only one. It also keeps the crates' configuration disjoint by construction: `qemer-core` knows an embedding endpoint, `qemer-answer` knows a completion endpoint, and neither names the other.

**The config type lives in the binary.** `qemer-tui` loads `config.toml` and hands each library crate its own parameters. A single shared `Config` in `qemer-core` would give core a field named for generation, and a struct field is a reference — the future `qemer-mcp`, which links core alone, would inherit a field it must ignore. The duplication this costs is a few lines; what it buys is that the boundary is enforced by the compiler rather than by remembering. Location is XDG via the `directories` crate: `~/.config/qemer/config.toml`, corpora under `~/.cache/qemer/corpora/`, with `QEMER_CONFIG` to override the file path.

**`k = 5`, with the token budget authoritative.** `k` and the context budget are two names for one constraint and only one can be in charge. `k` is a retrieval hint: fetch `k` candidates, then fill the prompt in rank order until the budget is spent and drop the tail. Snippet count therefore varies per query, which is correct — a query matching one huge code block and one matching five one-liners should not be forced to the same shape. Both values are config-settable, and the budget is derived from the model's actual context length read from configuration, never hardcoded. The starting value of 5 is a placeholder with no evidence behind it yet.

**Generation blocks input; Esc aborts.** A new query cannot be submitted while a stream is in flight, which means there is never a second stream and therefore nothing to interleave — no epoch counter or token-discarding logic is needed. Esc drops the in-flight request, so `llama-server` stops generating and the input line unlocks; without it there would be no way out of a bad answer from a small model, which is exactly the moment a user wants to retype.

**FTS uses the `code` base tokenizer, not the default.** Observed with `lancedb::tokenize` against lancedb 0.37.1: the default `simple` tokenizer stems and splits on underscores, indexing `create_index` as `creat` + `index` and dropping the `to` in `nearest_to` as a stop word; camelCase is never split, so `FullTextSearchQuery` becomes one stemmed blob either way. The `code` tokenizer keeps `create_index` and `nearest_to` whole while still splitting prose on whitespace. The cost is that BM25 loses stemming, so `creates` no longer matches `create`. That is the right trade in a hybrid: matching across word forms is what the vector retriever is for, and exact identifier matching is the one thing only BM25 can do. Both sides read `schema::fts_index_params()`, because index-time and query-time tokenization must agree.

**Arrow types come from `lancedb`'s re-exports, never a direct `arrow-*` dependency.** lancedb 0.37.1 depends on `arrow-array = "58.0.0"`. Declaring our own `arrow-array = "59.2.0"` put two versions in the graph, which is legal right up to the point a `RecordBatch` crosses into `create_table` and fails a trait bound on two identically named, incompatible types. Importing from `lancedb::arrow::*` makes the skew unrepresentable. `parquet` is not re-exported and must still be pinned by hand to match.

**One FTS index per column.** `lancedb` 0.37.1 rejects composite indices outright ("Multi-column (composite) indices are not yet supported"). Each column in `schema::FTS_COLUMNS` gets its own index, and a query searches across them with `FullTextSearchQuery::with_columns`.

**We fuse ourselves; `execute_hybrid` is not used.** lancedb 0.37.1 does expose a single-call hybrid (`VectorQuery::execute_hybrid`, which runs both searches and reranks with `RRFReranker`), so the earlier inference from `rerank_hybrid`'s signature was wrong on that point. It is still not what we want: it normalizes and fuses at the *row* level, and a corpus with a prose row and a code row per snippet would have one snippet vote twice. Each retriever is collapsed to distinct snippets first, and `fuse::rrf` combines the two snippet rankings.

## The corpus contract

The only thing this repository and `qemer-ingest` share. Nothing beyond this should be assumed on either side.

A manifest at a known URL lists what is available:

```json
{
  "corpora": [
    {
      "library": "lancedb",
      "version": "0.37.1",
      "url": "https://<host>/lancedb-0.37.1.tar.zst",
      "sha256": "...",
      "bytes": 15728640,
      "embedding_model": "nomic-embed-text-v1.5",
      "embedding_dim": 768,
      "snippet_count": 4213
    }
  ]
}
```

The client downloads, verifies the checksum, verifies the embedding model and dimension against its own configuration, and unpacks into a local cache.

The tarball contains Parquet, one row per prose-or-code unit:

| column | type | notes |
| --- | --- | --- |
| `snippet_id` | string | groups the prose and code rows of one block |
| `kind` | string | `prose` or `code` |
| `title` | string | the `###` heading |
| `source_url` | string | carried through from the Context7 block |
| `text` | string | the prose, or the code, per `kind` |
| `vector` | list<float32>[768] | embedding of `text` |

A block with no code contributes one row. Nothing beyond this table and the manifest above is shared with `qemer-ingest`.

## Open questions

Unresolved. **Ask rather than assume** — each of these has more than one defensible answer, and picking one silently is the specific failure this document exists to prevent.

- **Cache eviction.** Nothing decides when a downloaded corpus is removed, or whether that is ever automatic.
- **Token budget default.** `k = 5` is settled as a retrieval hint, but the budget it feeds is not chosen, and it depends on a context length that must be read from the model rather than assumed.
- **Failure surface for a missing `llama-server`.** Settled that Qemer says what to start. The two endpoints can be down independently, and what each message says is not written.

## Specifics to verify before relying on them

Named here because they were assumed during design and not checked against a running system.

- Whether `llama-server` requires, ignores, or rejects a per-request `model` field when a single model is loaded — this decides what the single-server deployment actually looks like in config docs.
- Qwen3.5-0.8B's real context length, read from the model card or the `llama-server` startup log, not from memory.

## Not in scope

Documentation scraping, corpus building, and publishing all live in `qemer-ingest` — including which libraries have corpora at all. This repository does not choose a library set; it consumes whatever the manifest lists. Catalog browsing, corpus update policy, and artifact hosting are out of scope. Auto-installing or launching llama.cpp is not planned. Cross-library query routing is deferred, not rejected.
