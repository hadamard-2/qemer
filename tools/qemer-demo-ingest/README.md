# Qemer demo corpus ingestion

This tool builds the NumPy and PyTorch demo-corpus assets from local snapshots. It only stages files locally; it never downloads documentation, starts a model server, or publishes a release.

Start a user-managed embedding server in one terminal:

```bash
# Terminal 1: the user starts the model server; the tool never does this.
/home/eyob-g/.local/bin/llama-server --model /home/eyob-g/Downloads/nomic-embed-text-v1.5.f16.gguf --embedding --port 8080 --no-webui
```

Build local, release-ready assets in a second terminal. The output directory must not already exist.

```bash
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

Safe consistency correction: the source plan’s command contained a duplicated `--embedding-dim` line, so this README documents that required option once.

The completed staging directory contains these files:

```text
manifest.json
numpy-2026-08-24.tar.zst
pytorch-2026-08-24.tar.zst
```

## Manual publication — one-way door

The following command creates publicly downloadable release assets. Correcting their names, URLs, or contents later requires replacing or deleting the published release, so inspect the staged manifest and archives before running it. This command is intentionally manual and is not part of the tool or its automated tests.

```bash
gh release create demo-2026-08-25 \
  /tmp/qemer-demo-corpora/manifest.json \
  /tmp/qemer-demo-corpora/numpy-2026-08-24.tar.zst \
  /tmp/qemer-demo-corpora/pytorch-2026-08-24.tar.zst \
  --repo OWNER/qemer-corpora \
  --title "Qemer demo corpora 2026-08-25"
```
