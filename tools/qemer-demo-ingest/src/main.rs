use std::path::{Path, PathBuf};

use clap::Parser;
use qemer_demo_ingest::{
    IngestError,
    embed::EmbeddingClient,
    package::{CorpusIdentity, ManifestEntry, stage_corpus, write_manifest},
    snapshot::parse_snapshot,
};

#[derive(Debug, Parser)]
#[command(
    name = "qemer-demo-ingest",
    about = "Build the NumPy and PyTorch demo corpora from local snapshots"
)]
struct Args {
    #[arg(long)]
    numpy: PathBuf,
    #[arg(long)]
    pytorch: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    asset_base_url: String,
    #[arg(long)]
    embedding_url: String,
    #[arg(long)]
    embedding_model: String,
    #[arg(long)]
    embedding_dim: usize,
}

const NUMPY_VERSION: &str = "2026-08-24";
const PYTORCH_VERSION: &str = "2026-08-24";

#[derive(Debug, Clone)]
struct InputCorpus {
    library: &'static str,
    version: &'static str,
    snapshot: PathBuf,
}

fn fixed_corpora(numpy: PathBuf, pytorch: PathBuf) -> [InputCorpus; 2] {
    [
        InputCorpus {
            library: "numpy",
            version: NUMPY_VERSION,
            snapshot: numpy,
        },
        InputCorpus {
            library: "pytorch",
            version: PYTORCH_VERSION,
            snapshot: pytorch,
        },
    ]
}

fn validate_output_dir(path: &Path) -> Result<(), IngestError> {
    if path.exists() {
        return Err(IngestError::OutputAlreadyExists {
            path: path.display().to_string(),
        });
    }
    Ok(())
}

async fn run(args: Args) -> Result<(), IngestError> {
    validate_output_dir(&args.output)?;
    std::fs::create_dir(&args.output)?;

    let embedding = EmbeddingClient::new(
        args.embedding_url,
        args.embedding_model.clone(),
        args.embedding_dim,
    );
    let mut entries: Vec<ManifestEntry> = Vec::new();
    for corpus in fixed_corpora(args.numpy, args.pytorch) {
        let snapshot = std::fs::read_to_string(&corpus.snapshot)?;
        let parsed = parse_snapshot(corpus.library, corpus.version, &snapshot)?;
        let rows = embedding.embed_all(parsed.text_units()).await?;
        let identity = CorpusIdentity {
            library: corpus.library.into(),
            version: corpus.version.into(),
            embedding_model: args.embedding_model.clone(),
            embedding_dim: args.embedding_dim,
            asset_base_url: args.asset_base_url.clone(),
        };
        entries.push(stage_corpus(&args.output, &identity, &rows)?);
    }
    write_manifest(&args.output, &entries)
}

#[tokio::main]
async fn main() -> Result<(), IngestError> {
    run(Args::parse()).await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use sha2::{Digest, Sha256};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        task::JoinHandle,
    };

    const NUMPY_SNAPSHOT: &str = "### NumPy example\n\nSource: https://example.test/numpy\n\nNumPy prose.\n--------------------------------\n";
    const PYTORCH_SNAPSHOT: &str = "### PyTorch example\n\nSource: https://example.test/pytorch\n\nPyTorch prose.\n--------------------------------\n";

    async fn read_request_body(stream: &mut TcpStream) -> serde_json::Value {
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        let header_end = loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert_ne!(
                read, 0,
                "connection closed before request headers completed"
            );
            request.extend_from_slice(&buffer[..read]);
            if let Some(position) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = std::str::from_utf8(&request[..header_end]).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        while request.len() < header_end + content_length {
            let read = stream.read(&mut buffer).await.unwrap();
            assert_ne!(read, 0, "connection closed before request body completed");
            request.extend_from_slice(&buffer[..read]);
        }
        serde_json::from_slice(&request[header_end..header_end + content_length]).unwrap()
    }

    async fn write_embedding_response(stream: &mut TcpStream, vector: &[f32]) {
        let body = serde_json::json!({ "data": [{ "embedding": vector }] }).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
    }

    async fn embedding_server(
        vectors: Vec<Vec<f32>>,
    ) -> (String, JoinHandle<Vec<serde_json::Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut requests = Vec::new();
            for vector in vectors {
                let (mut stream, _) = listener.accept().await.unwrap();
                requests.push(read_request_body(&mut stream).await);
                write_embedding_response(&mut stream, &vector).await;
            }
            requests
        });
        (format!("http://{address}"), server)
    }

    fn write_snapshots(root: &Path) -> (PathBuf, PathBuf) {
        let numpy = root.join("numpy.txt");
        let pytorch = root.join("pytorch.txt");
        std::fs::write(&numpy, NUMPY_SNAPSHOT).unwrap();
        std::fs::write(&pytorch, PYTORCH_SNAPSHOT).unwrap();
        (numpy, pytorch)
    }

    fn args(numpy: PathBuf, pytorch: PathBuf, output: PathBuf, embedding_url: String) -> Args {
        Args {
            numpy,
            pytorch,
            output,
            asset_base_url: "https://downloads.example.test/demo".into(),
            embedding_url,
            embedding_model: "required-model-stamp".into(),
            embedding_dim: 3,
        }
    }

    fn archive_members(path: &Path) -> Vec<PathBuf> {
        let decoder = zstd::stream::read::Decoder::new(std::fs::File::open(path).unwrap()).unwrap();
        tar::Archive::new(decoder)
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().into_owned())
            .collect()
    }

    fn independently_computed_sha256(path: &Path) -> String {
        format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
    }

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

    #[tokio::test]
    async fn run_stages_both_corpora_and_complete_manifest_with_required_model_stamp() {
        let root = tempfile::tempdir().unwrap();
        let (numpy, pytorch) = write_snapshots(root.path());
        let output = root.path().join("staged");
        let (embedding_url, server) =
            embedding_server(vec![vec![1.0, 2.0, 3.0], vec![4.0, 5.0, 6.0]]).await;

        run(args(numpy, pytorch, output.clone(), embedding_url))
            .await
            .unwrap();
        let requests = server.await.unwrap();

        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0]["input"], "NumPy prose.");
        assert_eq!(requests[1]["input"], "PyTorch prose.");
        assert!(
            requests
                .iter()
                .all(|request| request["model"] == "required-model-stamp")
        );

        let mut output_names = std::fs::read_dir(&output)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        output_names.sort();
        assert_eq!(
            output_names,
            vec![
                "manifest.json",
                "numpy-2026-08-24.tar.zst",
                "pytorch-2026-08-24.tar.zst"
            ]
        );

        let numpy_archive = output.join("numpy-2026-08-24.tar.zst");
        let pytorch_archive = output.join("pytorch-2026-08-24.tar.zst");
        assert_eq!(
            archive_members(&numpy_archive),
            vec![PathBuf::from("corpus.parquet")]
        );
        assert_eq!(
            archive_members(&pytorch_archive),
            vec![PathBuf::from("corpus.parquet")]
        );

        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(output.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(
            manifest.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["corpora"]
        );
        let corpora = manifest["corpora"].as_array().unwrap();
        assert_eq!(corpora.len(), 2);
        let expected_fields = BTreeSet::from([
            "bytes",
            "embedding_dim",
            "embedding_model",
            "library",
            "sha256",
            "snippet_count",
            "url",
            "version",
        ]);
        for entry in corpora {
            assert_eq!(
                entry
                    .as_object()
                    .unwrap()
                    .keys()
                    .map(String::as_str)
                    .collect::<BTreeSet<_>>(),
                expected_fields
            );
            assert_eq!(entry["version"], "2026-08-24");
            assert_eq!(entry["embedding_model"], "required-model-stamp");
            assert_eq!(entry["embedding_dim"], 3);
            assert_eq!(entry["snippet_count"], 1);
        }
        assert_eq!(corpora[0]["library"], "numpy");
        assert_eq!(
            corpora[0]["url"],
            "https://downloads.example.test/demo/numpy-2026-08-24.tar.zst"
        );
        assert_eq!(
            corpora[0]["sha256"],
            independently_computed_sha256(&numpy_archive)
        );
        assert_eq!(
            corpora[0]["bytes"],
            std::fs::metadata(&numpy_archive).unwrap().len()
        );
        assert_eq!(corpora[1]["library"], "pytorch");
        assert_eq!(
            corpora[1]["url"],
            "https://downloads.example.test/demo/pytorch-2026-08-24.tar.zst"
        );
        assert_eq!(
            corpora[1]["sha256"],
            independently_computed_sha256(&pytorch_archive)
        );
        assert_eq!(
            corpora[1]["bytes"],
            std::fs::metadata(&pytorch_archive).unwrap().len()
        );
    }

    #[tokio::test]
    async fn run_withholds_manifest_when_the_second_archive_cannot_be_written() {
        let root = tempfile::tempdir().unwrap();
        let (numpy, pytorch) = write_snapshots(root.path());
        let output = root.path().join("staged");
        let numpy_archive = output.join("numpy-2026-08-24.tar.zst");
        let blocked_pytorch_archive = output.join("pytorch-2026-08-24.tar.zst");
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request_body(&mut stream).await;
                assert_eq!(request["model"], "required-model-stamp");
                if request_index == 1 {
                    assert!(numpy_archive.is_file());
                    std::fs::create_dir(&blocked_pytorch_archive).unwrap();
                }
                write_embedding_response(&mut stream, &[1.0, 2.0, 3.0]).await;
            }
        });

        let error = run(args(
            numpy,
            pytorch,
            output.clone(),
            format!("http://{address}"),
        ))
        .await
        .unwrap_err();
        server.await.unwrap();

        assert!(matches!(error, IngestError::Io(_)));
        assert!(output.join("numpy-2026-08-24.tar.zst").is_file());
        assert!(output.join("pytorch-2026-08-24.tar.zst").is_dir());
        assert!(!output.join("manifest.json").exists());
    }
}
