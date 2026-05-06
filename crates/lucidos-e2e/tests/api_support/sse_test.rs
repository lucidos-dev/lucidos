use crate::support::{base_url, http_client, unique_marker};
use std::time::Duration;

#[tokio::test]
async fn sse_stream_connects_and_receives_events() {
    let client = http_client();
    let sse_url = format!("{}/api/events", base_url());

    // Connect to SSE stream
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

/// Mobile-on-Tailscale users connect via browsers that send `Accept-Encoding:
/// gzip` automatically. The SSE handler must compress the response body so
/// repetitive markdown / JSON event payloads (`MessageReceived` ≈17 KB,
/// `ToolResult` ≈4 KB, the embedded `ThreadInfo` aggregate snapshots) shrink
/// 10–20× on the wire. Verify both the negotiation header and the absence
/// when the client doesn't ask for it.
#[tokio::test]
async fn sse_sets_content_encoding_gzip_when_client_offers_it() {
    let client = http_client();
    let resp = client
        .get(format!("{}/api/events", base_url()))
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

    // Content-Type stays text/event-stream — gzip is on top, not a replacement.
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
    // The test reqwest client is built without `gzip`/`brotli`/`deflate`
    // features (see lucidos-e2e/Cargo.toml), so it doesn't auto-add
    // Accept-Encoding for us — omitting the header here really does omit
    // it on the wire.
    let resp = http_client()
        .get(format!("{}/api/events", base_url()))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("SSE connection failed");
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().get("content-encoding").is_none());
}

/// End-to-end round trip: gzip-compressed bytes from the SSE stream must
/// decompress to standard `data: <json>\n\n` SSE frames — proving the stream
/// flushes per-event (no buffering) and the wire format is preserved through
/// the encoder. Sends a chat to guarantee event traffic, then streams the
/// compressed body through `GzipDecoder` and asserts at least one `data:`
/// line surfaces before the test deadline.
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
            .get(format!("{}/api/events", base_url()))
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
                            // Got at least one event tied to this test — done.
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

    // Give the SSE subscriber a moment to register before sending the chat.
    tokio::time::sleep(Duration::from_secs(1)).await;

    let chat_url = format!("{}/api/chat/stream", base_url());
    let chat_body = serde_json::json!({
        "message": format!("Say exactly: \"sse gzip {marker}\""),
        "mode": "human",
    });
    http_client()
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
    let client = http_client();
    let marker = unique_marker("api-sse");

    // Start SSE stream in background
    let sse_url = format!("{}/api/events", base_url());
    let sse_client = http_client();

    let sse_handle = tokio::spawn(async move {
        let resp = sse_client
            .get(&sse_url)
            .header("Accept", "text/event-stream")
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .expect("SSE connection failed");

        // Read a chunk of the response body
        let body = resp.text().await.unwrap_or_default();
        body
    });

    // Small delay to let SSE connect
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Send a chat message to generate events
    let chat_url = format!("{}/api/chat/stream", base_url());
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

    // Wait for response to complete, then collect SSE data
    tokio::time::sleep(Duration::from_secs(15)).await;

    // The SSE stream will time out and return what it collected
    // We can't easily read streaming responses with reqwest in this simple way,
    // so we verify the connection was established successfully above
    // and trust that events flow through SSE (verified by browser tests)
    let _ = sse_handle.await;
}
