# Qemer TUI: the query loop

**Status:** design approved, ready for an implementation plan.

**Scope:** the terminal application, limited to querying corpora that are *already installed*. Browsing the manifest, choosing versions, and evicting the cache are deliberately excluded — see "Deferred" below.

## Why this slice

The TUI is two subsystems, not one, and they are blocked on different things.

The query loop — pick an installed library, ask a question, read a streamed answer with its sources — is blocked on almost nothing. Every capability it needs already exists and is tested: `Cache::installed()`, `search::search()`, and `qemer_answer::Generator`.

Corpus browsing and install is blocked on three of the five open questions in [`docs/decisions.md`](../../decisions.md) — the install flow, version selection, and cache eviction. It also reaches back into `qemer-core`: `corpus::install()` downloads with `reqwest::get(...).bytes().await`, buffering the whole tarball in memory with no progress signal of any kind. Showing a download in progress therefore requires changing a crate that is currently finished and tested, streaming the response body and reporting progress out. That is a real piece of work, and it does not belong in the same spec as a screen that renders text.

Splitting them lets the product's reason to exist get built first, against corpora installed by a command-line path that settles none of the deferred design questions.

## Decisions this design makes

Each was chosen in discussion. The reasoning is recorded so overriding one is cheap.

**Single-turn, one live answer at a time.** The main screen shows one question and one answer; asking again replaces what was there. `Generator::answer()` is strictly single-turn — it builds exactly two messages and carries no prior turns — so a scrolling chat transcript would imply a conversational memory that does not exist, and a follow-up like "what about the async version?" would be retrieved and answered cold while looking like it had context. Real multi-turn would mean extending prompt assembly to carry history, which re-opens budgeting: history would compete with snippets for the same context window on a small model. Not now.

**A picker screen on startup, not a persistent selector.** The app opens on the list of installed corpora and selecting one enters the query screen. Two screens make switching libraries an obvious act rather than a hidden key, and the picker is the natural place for corpus management to grow when it is designed.

**Corpora are installed from the command line in this slice.** `qemer install <library>@<version>` and `qemer list` are non-interactive wrappers over `fetch_manifest` and `install` that print to stdout. The version is **required**, not optional — defaulting it to the newest available would be answering the version-selection question that `docs/decisions.md` records as open, and doing so silently in a command-line flag is exactly the quiet resolution that document exists to prevent. They are scriptable, testable without a terminal harness, and settle none of the deferred interaction questions. Building even a minimal install prompt inside the picker would bias the real design later, which is the specific thing this split exists to avoid.

**Context length is a required config key with no default.** `docs/decisions.md` says the budget is derived from the model's real context length "read from configuration, never hardcoded", and separately lists Qwen3.5-0.8B's actual context length as a fact to verify rather than recall. Shipping a conservative default would bake in exactly the unmeasured round number that both that document and `CLAUDE.md` warn about, and would silently waste context on a larger model. Startup therefore fails when the key is absent, naming the key and telling the user where to read the value.

An alternative was considered and rejected: reading the context length from the running `llama-server` at startup, which would make the number measured rather than trusted. It was rejected because it depends on an endpoint whose behaviour was not verified, and because the required-key design removes the dependency entirely. If a future version wants it, this is where to look.

**Reachability is checked at the point of use, never probed at startup.** A server can die mid-session regardless of what a startup probe found, so use-time handling is mandatory in every possible design; a probe is purely additive and can be stale within seconds. The two endpoints fail independently and are reported independently — a failed search names the embedding endpoint, a failed generation names the completion endpoint.

**One unified query stream, cancelled by drop.** Generation is a stream but search is a single `await` — one embedding round-trip plus a database query. Awaiting it directly would freeze the interface for its duration, including through a network timeout when the embedding server is down, which is precisely the moment a user wants to escape. Wrapping both phases in one stream gives search the same escape hatch generation already has, and preserves the contract `qemer-answer` was built around: dropping the stream is the whole cancellation story. The alternative — a spawned task posting messages down a channel — degrades cancellation to `JoinHandle::abort()`, abandoning a task at an arbitrary await point instead of closing the connection, and re-implements in channel form a contract that already exists.

## Module layout

| File | Responsibility |
| --- | --- |
| `main.rs` | Entry point. Dispatches between the TUI and the `install` / `list` subcommands. Installs the panic hook. |
| `config.rs` | The `Config` type, TOML loading, XDG resolution with `QEMER_CONFIG` override, and validation. Pure apart from the file read. |
| `cli.rs` | `install` and `list`. Non-interactive, prints to stdout, never touches ratatui. |
| `app.rs` | Application state and the `select!` event loop. Key handling is a pure state transition. |
| `query.rs` | The unified query stream, and the mapping from crate errors to user-facing failures. |
| `view.rs` | Rendering only. Takes `&App` and draws. Holds no state and performs no I/O. |

`view.rs` holding no state is what makes the application testable without a terminal: every decision worth asserting lives in `app.rs`, `config.rs`, or `query.rs`, none of which need a screen.

## Configuration

The config type lives in the binary and hands each library crate its own parameters, per `docs/decisions.md`. A shared `Config` in `qemer-core` would give core a field named for generation, and the future `qemer-mcp` — which links core alone — would inherit a field it must ignore.

```toml
manifest_url = "https://<host>/manifest.json"

[embedding]
base_url = "http://localhost:8080"
model    = "nomic-embed-text-v1.5"
dim      = 768

[completion]
base_url              = "http://localhost:8081"
model                 = "qwen3.5-0.8b"
# Both required, both deliberately unfilled here. Read context_tokens from
# your llama-server startup log; no value is suggested, because suggesting
# one is how a placeholder becomes a fact by repetition.
context_tokens        = <your model's real context length>
max_completion_tokens = <how much room to leave for the answer>

[retrieval]
k = 5
```

The two `[completion]` values above are shown unfilled on purpose: no number is proposed for either, here or anywhere in the implementation. `context_tokens`, `max_completion_tokens`, and `manifest_url` are required and have no defaults — the first two for the reason given above, and `manifest_url` because no host has been chosen yet. Everything else defaults to the values shown. The file is read from `~/.config/qemer/config.toml` via the `directories` crate, overridable with the `QEMER_CONFIG` environment variable.

Validation failures are reported before the terminal is put into raw mode, so the message survives on screen.

## Screens

### Picker

Lists what `Cache::installed()` returns, one row per corpus: library, version, and snippet count. `↑`/`↓` and `j`/`k` move, `Enter` selects, `q` quits.

The empty state is load-bearing in this slice, because a fresh user has no corpora and the application offers no way to get one. It states the exact command to run rather than describing it.

### Query

The header shows the active library and version. The input line sits at the bottom, the answer fills the body, and the sources are listed beneath it, numbered, each with its title and source URL.

That weighting follows `docs/decisions.md`'s "conversational answers, for now" — the generated answer is the primary content with snippets available beneath. If the sources-first fallback is ever taken, it changes the prompt, the token cap, and which pane gets the space; the structure here does not change.

### Keys

`Esc` is deliberately overloaded on the query screen, resolved by state: while a stream is in flight it aborts, and while idle it returns to the picker. `Enter` submits only when idle, because generation blocks input — which is why there is never a second stream and nothing to interleave.

## Data flow

```rust
enum QueryEvent {
    Searching,
    Snippets(Vec<Snippet>),
    Token(String),
    Done { prompt_tokens: usize, completion_tokens: usize },
}
```

`query::run(...)` returns a stream of these. The application holds an `Option<QueryStream>` and selects over it alongside the terminal event stream. Aborting is setting that field to `None`; the drop cancels whichever phase is live.

`Snippets` is emitted before the first token, so the grounding appears while the model is still working. This matters most exactly when the answer turns out to be wrong — the sources are what let a user notice.

## Error handling

`QueryError` distinguishes the two endpoints so each message names the one that actually failed and says what to start. This is the crate boundary paying off: `AnswerError::Unreachable` carries the URL and no advice specifically so that the caller, which knows which endpoint it wanted, supplies it.

Errors render into the answer pane. Nothing about a failed query terminates the application. A panic hook calls `ratatui::restore()` before unwinding, so a crash never leaves the terminal in raw mode.

## Dependencies

Two changes to `qemer-tui/Cargo.toml`, both verified against the crates on disk rather than recalled.

**`crossterm` needs the `event-stream` feature.** `EventStream` is gated behind it (`crossterm-0.29.0/src/event.rs:124`) and the feature is not in crossterm's defaults. The crate currently declares `crossterm = "0.29.0"` with no features, so the event loop described here does not compile as things stand. Neither `ratatui` nor `ratatui-crossterm` exposes a passthrough for it, so the feature must be enabled on a direct dependency; Cargo's feature unification then applies it to the single shared crossterm in the graph.

**Crossterm types are imported through `ratatui::crossterm`.** ratatui 0.30.2 re-exports it (`ratatui-0.30.2/src/lib.rs:483`) and selects the version through its `crossterm_0_29` feature. This is the same hazard `docs/decisions.md` already ruled on for Arrow: a direct dependency can drift from what ratatui builds against, producing two identically named and mutually incompatible `KeyEvent` types. The direct dependency exists only to carry the feature flag; every `use` goes through the re-export. The lockfile currently resolves exactly one crossterm 0.29.0, so there is no skew today — this keeps it that way.

**`clap` is added for argument parsing.** Two subcommands do not require it — matching on `std::env::args` would be roughly twenty lines and no new dependency — but the command-line surface grows when corpus management lands, and `--help` comes free. This is the cheapest decision here to reverse.

## Testing

Per `CLAUDE.md`, no terminal harness for logic that never touches a terminal. Every item below is a pure function over data.

- **`config.rs`** — a missing required key names that key; `QEMER_CONFIG` overrides the XDG path; defaults apply only where a default exists.
- **`query.rs`** — the error mapping. A `CoreError::Embed` must produce advice about the embedding endpoint and must never mention the completion endpoint, and the reverse for `AnswerError::Unreachable`. This is the test that keeps the two endpoints from blurring into one message.
- **`app.rs`** — `handle_key` as a state transition: `Enter` is ignored while a stream is in flight; `Esc` aborts while streaming and navigates while idle; the picker's selection index does not run off either end of the list.
- **`view.rs`** — one or two `TestBackend` assertions, used sparingly, for layout that genuinely cannot be asserted any other way.

## Deferred

Out of scope here, each needing its own design.

- **Corpus browsing and install in the interface**, including how the manifest list is presented and searched, what an install looks like mid-download, and how multiple versions of one library are shown. Blocked on three open questions in `docs/decisions.md`, and on giving `corpus::install()` a streaming download with progress reporting.
- **Version selection and cache eviction.** Open in `docs/decisions.md`, and they interact with each other.
- **Multi-turn conversation.** Requires prompt assembly to carry history and re-opens context budgeting.
- **Measuring whether the retrieval defaults are any good.** `k = 5` is a placeholder with no evidence behind it, as `docs/decisions.md` says.
