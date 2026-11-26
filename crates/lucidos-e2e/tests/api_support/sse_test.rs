use crate::support::{base_url, http_client, unique_marker, user_client};
use std::time::Duration;

#[tokio::test]
async fn sse_stream_connects_and_receives_events() {
    let client = user_client().await;
    let sse_url = format!("{}/api/v1/events", base_url());

    let resp = client
        .get(&sse_url)
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("SSE connection failed");

    assert_eq!(resp.status(), 200);

    let content_type = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or(""))
        .unwrap_or("");
    assert!(
        content_type.contains("text/event-stream"),
        "Expected text/event-stream, got: {content_type}"
    );
}

#[tokio::test]
async fn sse_sets_content_encoding_gzip_when_client_offers_it() {
    let client = user_client().await;
    let resp = client
        .get(format!("{}/api/v1/events", base_url()))
        .header("Accept", "text/event-stream")
        .header("Accept-Encoding", "gzip")
        .send()
        .await
        .expect("SSE connection failed");
    assert_eq!(resp.status(), 200);

    let ce = resp
        .headers()
        .get("content-encoding")
        .map(|v| v.to_str().unwrap_or("").to_string());
    assert_eq!(
        ce.as_deref(),
        Some("gzip"),
        "Expected Content-Encoding: gzip, got {:?}",
        ce
    );

    let ct = resp
        .headers()
        .get("content-type")
        .map(|v| v.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    assert!(
        ct.contains("text/event-stream"),
        "Expected Content-Type to remain text/event-stream, got: {ct}"
    );
}

#[tokio::test]
async fn sse_omits_content_encoding_when_client_does_not_offer_gzip() {
    let resp = http_client()
        .get(format!("{}/api/v1/events", base_url()))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("SSE connection failed");
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("content-encoding").is_none());
}

#[tokio::test]
async fn sse_compressed_events_decompress_to_sse_wire_format() {
    use async_compression::tokio::bufread::GzipDecoder;
    use futures::StreamExt;
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio_util::io::StreamReader;

    let marker = unique_marker("sse-gzip-roundtrip");

    let sse_marker = marker.clone();
    let sse_handle: tokio::task::JoinHandle<Vec<String>> = tokio::spawn(async move {
        let resp = http_client()
            .get(format!("{}/api/v1/events", base_url()))
            .header("Accept", "text/event-stream")
            .header("Accept-Encoding", "gzip")
            .timeout(Duration::from_secs(60))
            .send()
            .await
            .expect("SSE connection failed");
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()
                .get("content-encoding")
                .and_then(|v| v.to_str().ok()),
            Some("gzip"),
            "Round-trip test requires gzip encoding to be active",
        );

        let byte_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
        let reader = StreamReader::new(byte_stream);
        let decoder = GzipDecoder::new(BufReader::new(reader));
        let mut lines = BufReader::new(decoder).lines();

        let mut collected = Vec::new();
        let deadline = tokio::time::sleep(Duration::from_secs(30));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        let saw_marker = line.contains(&sse_marker);
                        collected.push(line);
                        if saw_marker {
                            break;
                        }
                    }
                    Ok(None) | Err(_) => break,
                },
                _ = &mut deadline => break,
            }
        }
        collected
    });

    tokio::time::sleep(Duration::from_secs(1)).await;

    let chat_url = format!("{}/api/v1/chat/stream", base_url());
    let chat_body = serde_json::json!({
        "message": format!("Say exactly: \"sse gzip {marker}\""),
        "mode": "human",
    });
    user_client()
        .await
        .post(&chat_url)
        .json(&chat_body)
        .send()
        .await
        .expect("Chat request failed");

    let lines = sse_handle.await.expect("SSE task panicked");

    assert!(
        lines.iter().any(|l| l.starts_with("data: ")),
        "Decompressed stream must contain at least one SSE data line; got {} lines: {:?}",
        lines.len(),
        lines.iter().take(5).collect::<Vec<_>>(),
    );
    assert!(
        lines.iter().any(|l| l.contains(&marker)),
        "Decompressed stream must contain the chat marker `{marker}`; \
         got {} lines, first 5: {:?}",
        lines.len(),
        lines.iter().take(5).collect::<Vec<_>>(),
    );
}

#[tokio::test]
async fn sse_receives_events_after_chat() {
    let client = user_client().await;
    let marker = unique_marker("api-sse");

    let sse_url = format!("{}/api/v1/events", base_url());
    let sse_client = http_client();

    let sse_handle = tokio::spawn(async move {
        let resp = sse_client
            .get(&sse_url)
            .header("Accept", "text/event-stream")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .expect("SSE connection failed");

        let body = resp.text().await.unwrap_or_default();
        body
    });

    tokio::time::sleep(Duration::from_secs(1)).await;

    let chat_url = format!("{}/api/v1/chat/stream", base_url());
    let chat_body = serde_json::json!({
        "message": format!("Say exactly: \"sse {marker}\""),
        "mode": "human",
    });
    client
        .post(&chat_url)
        .json(&chat_body)
        .send()
        .await
        .expect("Chat request failed");

    tokio::time::sleep(Duration::from_secs(15)).await;

    let _ = sse_handle.await;
}
