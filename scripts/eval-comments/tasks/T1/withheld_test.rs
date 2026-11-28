use super::*;

fn sse_response(body: impl Into<reqwest::Body>) -> reqwest::Response {
    reqwest::Response::from(axum::http::Response::new(body.into()))
}

fn truncated_tool_args_named(tool_name: &str, stop_reason: Option<&str>) -> String {
    let mut body = format!(
        concat!(
            r#"data: {{"type":"message_start","message":{{"usage":{{"input_tokens":112000}}}}}}"#,
            "\n\n",
            r#"data: {{"type":"content_block_start","index":0,"#,
            r#""content_block":{{"type":"tool_use","id":"tu_1","name":"{}"}}}}"#,
            "\n\n",
            r#"data: {{"type":"content_block_delta","index":0,"#,
            r#""delta":{{"type":"input_json_delta","#,
            r#""partial_json":"{{\"path\": \"artifacts/research/architecture.md\""}}}}"#,
            "\n\n"
        ),
        tool_name
    );
    if let Some(reason) = stop_reason {
        body.push_str(&format!(
            r#"data: {{"type":"message_delta","delta":{{"stop_reason":"{reason}"}},"#
        ));
        body.push_str(r#""usage":{"output_tokens":128000}}"#);
        body.push_str("\n\n");
    }
    body
}

#[tokio::test]
async fn a_tool_named_like_an_http_status_cannot_flip_the_classification() {
    for name in ["502", "503", "529"] {
        for stop in ["max_tokens", "tool_use"] {
            let err = parse_claude_stream(
                sse_response(truncated_tool_args_named(name, Some(stop))),
                &None,
                "Test",
            )
            .await
            .expect_err("unparseable arguments are still an error");

            let msg = err.to_string();
            assert!(
                !crate::llm::is_retryable_error(&msg),
                "tool '{name}' at stop '{stop}' must stay non-retryable, got: {msg}"
            );
        }
    }
}
