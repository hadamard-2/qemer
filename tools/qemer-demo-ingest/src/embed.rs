use crate::{IngestError, snapshot::TextUnit};

#[derive(Debug, Clone)]
pub struct EmbeddedUnit {
    pub unit: TextUnit,
    pub vector: Vec<f32>,
}

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
    struct Response {
        data: Vec<Datum>,
    }

    #[derive(serde::Deserialize)]
    struct Datum {
        embedding: Vec<f32>,
    }

    let response: Response = serde_json::from_slice(body)
        .map_err(|error| IngestError::Embed(format!("invalid embeddings response: {error}")))?;
    let vector = response
        .data
        .into_iter()
        .next()
        .ok_or_else(|| IngestError::Embed("response contained no embeddings".into()))?
        .embedding;
    if vector.len() != expected_dim {
        return Err(IngestError::Embed(format!(
            "expected {expected_dim} dimensions, received {}",
            vector.len()
        )));
    }
    Ok(vector)
}

impl EmbeddingClient {
    pub fn new(base_url: impl Into<String>, model: impl Into<String>, dimension: usize) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url.into(),
            model: model.into(),
            dimension,
        }
    }

    pub async fn embed_one(&self, unit: TextUnit) -> Result<EmbeddedUnit, IngestError> {
        let url = format!("{}/v1/embeddings", self.base_url.trim_end_matches('/'));
        let response = self
            .http
            .post(&url)
            .json(&embedding_request(&unit.text, &self.model))
            .send()
            .await
            .map_err(|error| {
                IngestError::Embed(format!("embedding request to {url} failed: {error}"))
            })?
            .error_for_status()
            .map_err(|error| {
                IngestError::Embed(format!("embedding request to {url} failed: {error}"))
            })?;
        let body = response.bytes().await.map_err(|error| {
            IngestError::Embed(format!("embedding request to {url} failed: {error}"))
        })?;
        let vector = parse_embedding(&body, self.dimension).map_err(|error| {
            IngestError::Embed(format!("embedding response from {url} failed: {error}"))
        })?;
        Ok(EmbeddedUnit { unit, vector })
    }

    pub async fn embed_all(&self, units: Vec<TextUnit>) -> Result<Vec<EmbeddedUnit>, IngestError> {
        let mut embedded = Vec::with_capacity(units.len());
        for unit in units {
            embedded.push(self.embed_one(unit).await?);
        }
        Ok(embedded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

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
        assert_eq!(body.as_object().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn embeds_one_unit_over_the_openai_compatible_wire_shape() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut read_buffer = [0; 1024];
            let header_end = loop {
                let read = stream.read(&mut read_buffer).await.unwrap();
                request.extend_from_slice(&read_buffer[..read]);
                if let Some(position) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    break position + 4;
                }
            };
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let content_length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            while request.len() < header_end + content_length {
                let read = stream.read(&mut read_buffer).await.unwrap();
                request.extend_from_slice(&read_buffer[..read]);
            }
            let body: serde_json::Value =
                serde_json::from_slice(&request[header_end..header_end + content_length]).unwrap();
            assert_eq!(body["input"], "some code");
            assert_eq!(body["model"], "nomic-embed-text-v1.5");

            let body = r#"{"data":[{"embedding":[1.0,2.0,3.0]}]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len(),
            );
            stream.write_all(response.as_bytes()).await.unwrap();
        });
        let client = EmbeddingClient::new(format!("http://{address}"), "nomic-embed-text-v1.5", 3);
        let unit = TextUnit {
            snippet_id: "unit-1".into(),
            kind: "code",
            title: "Example".into(),
            source_url: "https://example.test".into(),
            text: "some code".into(),
        };

        let embedded = client.embed_one(unit).await.unwrap();
        assert_eq!(embedded.vector, vec![1.0, 2.0, 3.0]);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn embed_all_preserves_order_and_stops_at_the_first_failure() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut inputs = Vec::new();
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut read_buffer = [0; 1024];
                let header_end = loop {
                    let read = stream.read(&mut read_buffer).await.unwrap();
                    request.extend_from_slice(&read_buffer[..read]);
                    if let Some(position) =
                        request.windows(4).position(|bytes| bytes == b"\r\n\r\n")
                    {
                        break position + 4;
                    }
                };
                let headers = std::str::from_utf8(&request[..header_end]).unwrap();
                let content_length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                while request.len() < header_end + content_length {
                    let read = stream.read(&mut read_buffer).await.unwrap();
                    request.extend_from_slice(&read_buffer[..read]);
                }
                let body: serde_json::Value =
                    serde_json::from_slice(&request[header_end..header_end + content_length])
                        .unwrap();
                inputs.push(body["input"].as_str().unwrap().to_owned());

                let (status, body) = if request_index == 0 {
                    ("200 OK", r#"{"data":[{"embedding":[1.0,2.0,3.0]}]}"#)
                } else {
                    ("500 Internal Server Error", r#"{"error":"stop"}"#)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
            inputs
        });
        let client = EmbeddingClient::new(format!("http://{address}"), "model", 3);
        let units = ["first", "second", "third"]
            .into_iter()
            .enumerate()
            .map(|(index, text)| TextUnit {
                snippet_id: format!("unit-{index}"),
                kind: "prose",
                title: "Example".into(),
                source_url: "https://example.test".into(),
                text: text.into(),
            })
            .collect();

        let error = client.embed_all(units).await.unwrap_err();
        assert!(error.to_string().contains("500 Internal Server Error"));
        assert_eq!(server.await.unwrap(), vec!["first", "second"]);
    }
}
