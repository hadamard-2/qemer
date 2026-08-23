mod completion_stub;

use futures::StreamExt;
use qemer_answer::{AnswerEvent, Generator};
use qemer_core::Snippet;

fn a_snippet() -> Snippet {
    Snippet {
        library: "lancedb".into(),
        version: "0.37.1".into(),
        snippet_id: "s1".into(),
        title: "Full text search".into(),
        description: "Run a keyword search over an indexed table.".into(),
        code: Some("table.create_index(&[\"text\"], Index::FTS(params)).await?;".into()),
        source_url: Some("https://example/fts".into()),
        score: 1.0,
    }
}

fn generator(base_url: String) -> Generator {
    Generator {
        base_url,
        model: "qwen3.5-0.8b".into(),
        context_tokens: 8192,
        max_completion_tokens: 512,
    }
}

#[tokio::test]
async fn a_streamed_answer_yields_tokens_then_done_with_counts() {
    let frames = vec![
        r#"{"choices":[{"delta":{"role":"assistant"},"index":0}]}"#.to_string(),
        r#"{"choices":[{"delta":{"content":"Call "},"index":0}]}"#.to_string(),
        r#"{"choices":[{"delta":{"content":"create_index"},"index":0}]}"#.to_string(),
        r#"{"choices":[{"delta":{"content":"."},"index":0}]}"#.to_string(),
        r#"{"choices":[],"usage":{"prompt_tokens":44,"completion_tokens":3,"total_tokens":47}}"#
            .to_string(),
        "[DONE]".to_string(),
    ];
    let base_url = completion_stub::start(frames).await;
    let generator = generator(base_url);

    let events: Vec<_> = generator
        .answer("how do I search?", &[a_snippet()])
        .collect()
        .await;
    let events: Vec<AnswerEvent> = events.into_iter().map(|e| e.unwrap()).collect();

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            AnswerEvent::Token(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Call create_index.");

    match events.last().unwrap() {
        AnswerEvent::Done { prompt_tokens, completion_tokens } => {
            assert_eq!(*prompt_tokens, 44);
            assert_eq!(*completion_tokens, 3);
        }
        other => panic!("stream must end with Done, got {other:?}"),
    }
}

#[tokio::test]
async fn a_stream_without_usage_still_ends_cleanly() {
    let frames = vec![
        r#"{"choices":[{"delta":{"content":"Hi"},"index":0}]}"#.to_string(),
        "[DONE]".to_string(),
    ];
    let base_url = completion_stub::start(frames).await;

    let events: Vec<_> = generator(base_url)
        .answer("q", &[a_snippet()])
        .collect()
        .await;
    let last = events.into_iter().last().unwrap().unwrap();
    assert!(
        matches!(last, AnswerEvent::Done { prompt_tokens: 0, completion_tokens: 0 }),
        "a server that reports no usage must still terminate the stream"
    );
}

#[tokio::test]
async fn an_unreachable_server_surfaces_as_unreachable() {
    // Port 1 on loopback: reserved, and nothing will be listening.
    let generator = generator("http://127.0.0.1:1".into());
    let events: Vec<_> = generator.answer("q", &[a_snippet()]).collect().await;
    let first = events.into_iter().next().unwrap();
    assert!(matches!(
        first,
        Err(qemer_answer::AnswerError::Unreachable(_))
    ));
}
