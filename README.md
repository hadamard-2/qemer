# Qemer

Qemer is an offline coding assistant grounded in technical documentation. It searches documentation corpora installed on your computer, gives those retrieved excerpts to a local language model, and shows both the answer and the supporting sources.

## What you need

Qemer deliberately does not download, install, or run models for you. Before using it, you need a running [`llama-server`](https://github.com/ggml-org/llama.cpp/tree/master/tools/server) for embeddings and completions, plus a prebuilt documentation corpus available through a local or HTTPS manifest. One server can provide both endpoints when it serves both models.

The embedding model must match the corpus exactly. Each corpus records its embedding model and vector dimension; Qemer checks both before searching so it can reject a mismatch instead of returning misleading results.

## Installation

<details>
<summary><strong>Pre-built binary (Linux)</strong></summary>

The release archive is built for 64-bit GNU/Linux (`x86_64-unknown-linux-gnu`). Download the `.tar.gz` archive and its `.sha256` checksum from the [latest release](https://github.com/hadamard-2/qemer/releases/latest), then verify and install it:

```sh
curl -LO https://github.com/hadamard-2/qemer/releases/download/v0.1.0/qemer-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/hadamard-2/qemer/releases/download/v0.1.0/qemer-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
sha256sum -c qemer-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf qemer-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
mkdir -p ~/.local/bin
install -m 755 qemer-v0.1.0-x86_64-unknown-linux-gnu/qemer ~/.local/bin/qemer
```

Make sure `~/.local/bin` is on your `PATH`, then confirm the installation:

```sh
qemer --version
```

For a later release, replace `v0.1.0` in the commands with that release's tag and use the matching archive names.

</details>

<details>
<summary><strong>Build from source (Linux, macOS, and Windows)</strong></summary>

Building from source is supported on Linux, macOS, and Windows. Install [Rust](https://www.rust-lang.org/tools/install) and ensure `protoc` is on your `PATH`; Qemer needs the Protocol Buffers compiler while building its local search engine. On Debian or Ubuntu, install it with `sudo apt install protobuf-compiler`.

Then clone and install Qemer:

```sh
git clone https://github.com/hadamard-2/qemer.git
cd qemer
cargo install --path qemer-tui --locked
qemer --version
```

On macOS and Windows, use your usual package manager or the Protocol Buffers installation instructions to install `protoc`, then run the same Rust commands. The first build can take several minutes because it compiles Qemer's search dependencies.

</details>

## Usage

### 1. Start your model servers

Start the `llama-server` endpoint or endpoints you plan to use. Qemer uses one endpoint to turn your question into an embedding and another to generate the answer. The embedding endpoint must serve the same model and vector dimension recorded by the corpus you will install.

### 2. Configure Qemer

Run the interactive configuration wizard:

```sh
qemer config
```

Enter the embedding and completion server URLs, model names, and embedding dimension. For `completion.context_tokens`, use the `n_ctx_slot` value printed in your completion server's startup log. Choose `completion.max_completion_tokens` as the maximum length of an answer, no greater than the context size. Qemer saves this configuration to `~/.config/qemer/config.toml` by default.

### 3. Find and install a corpus

A manifest lists the available documentation corpora. It can be a local file or an HTTPS URL. First inspect it, then choose the exact library and version you want:

```sh
MANIFEST=/absolute/path/to/manifest.json
qemer available --manifest "$MANIFEST"
qemer install <library>@<version> --manifest "$MANIFEST"
qemer list
```

For example, if the manifest offers NumPy 2.5.2, install it with `qemer install numpy@2.5.2 --manifest "$MANIFEST"`. Qemer verifies the downloaded artifact's size and SHA-256 checksum before installing it locally.

### 4. Ask a question

Launch Qemer:

```sh
qemer
```

Choose an installed corpus with the arrow keys and press <kbd>Enter</kbd>. Type a question about that library and press <kbd>Enter</kbd> again. Qemer retrieves relevant documentation, streams a grounded answer, and lists the excerpts it used below the answer. Use the arrow keys to select an excerpt and press <kbd>Enter</kbd> on an empty prompt to read it; press <kbd>Esc</kbd> to return or abort a response in progress.

Queries are scoped to the corpus you select. If you need help with another library or version, return to the corpus list with <kbd>Esc</kbd> and choose the appropriate one.

## How it works

1. You install a prebuilt corpus that contains documentation text and embeddings.
2. Qemer embeds your question, searches the selected corpus locally with both semantic and exact-term search, and retrieves the best excerpts.
3. Your local completion model receives the question and excerpts, then streams an answer in the terminal.

This keeps retrieval local and gives you the source material behind each answer. It also means answer quality depends on the corpus and the models you run.

## Troubleshooting

- **"No config file"** — run `qemer config` to create one.
- **An endpoint cannot be reached** — start `llama-server` and confirm its URL matches the value saved by `qemer config`.
- **Embedding model or dimension mismatch** — configure the exact embedding model and dimension advertised by the corpus manifest, then run `qemer config` again.
- **No corpora installed** — use `qemer available --manifest <path-or-https-url>` and then install an explicit `library@version`.

## Corpus contract

Qemer consumes prebuilt corpora; it does not crawl documentation or create embeddings. The manifest and archive are the complete consumer contract:

```
Manifest → [ { library, version, url, sha256, bytes,
               embedding_model, embedding_dim, snippet_count } ]
```

The archive contains `corpus.parquet`, with one row for each prose-or-code unit. See [`docs/decisions.md`](docs/decisions.md) for the complete artifact layout and the project's settled design decisions.

## Development

Building requires `protoc` on `PATH` — `lance-encoding` compiles protobuf definitions in its build script. On Debian or Ubuntu:

```sh
sudo apt install protobuf-compiler
```

Then run:

```sh
cargo build
cargo test
```

The first build compiles DataFusion and can take several minutes.
