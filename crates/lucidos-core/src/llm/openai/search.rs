use futures::{Stream, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;

use super::responses::{http_failure_step, HttpFailureStep};
use super::{OpenAiProvider, CHUNK_TIMEOUT_SECS};
use crate::engine::thread_events::billing_provider::BillingProvider;
use crate::llm::chatgpt_plan::ChatGptPlanError;
use crate::llm::metered::CallToken;
use crate::llm::provider::{Message, MessageContent};
use crate::llm::web_search::{WebSearchResult, SEARCH_SYSTEM_PROMPT};
use crate::llm::{ModelSelection, ProviderKind};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

impl OpenAiProvider {
    pub(super) fn build_plan_search_body(&self, model: &str, query: &str) -> Value {
        let messages = [Message {
            role: "user".into(),
            content: MessageContent::Text(query.into()),
        }];
        let mut body = self.build_responses_body(
            model,
            &messages,
            &[],
            Some(SEARCH_SYSTEM_PROMPT),
            &ModelSelection::default(),
        );
        body["tools"] = json!([{"type": "web_search"}]);
        body
    }

    pub(crate) async fn search_plan(
        &self,
        model: &str,
        query: &str,
        max_results: usize,
        _call: CallToken,
    ) -> Result<WebSearchResult, BoxError> {
        let tokens = self
            .plan_tokens()
            .ok_or("ChatGPT plan search requires a plan endpoint")?;
        crate::llm::metered::check_provider(ProviderKind::ChatGptPlan).await?;
        let mut bearer = self
            .plan_bearer(model)
            .await?
            .ok_or("missing plan bearer")?;
        let body = self.build_plan_search_body(model, query);
        let mut refreshed = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let request = self
                .apply_headers(
                    self.streaming_client.post(&self.responses_url),
                    Some(&bearer),
                )
                .json(&body);
            let response =
                match crate::llm::send_streaming_request(request, model, attempt, None).await {
                    crate::llm::StreamSend::Got(response) => response,
                    crate::llm::StreamSend::Retry => continue,
                    crate::llm::StreamSend::Failed(error) => return Err(error),
                };
            let status = response.status();
            if !status.is_success() {
                let request_id = crate::llm::call_trace::request_id_of(response.headers());
                let error_headers = response.headers().clone();
                let error_body =
                    tokio::time::timeout(Duration::from_secs(CHUNK_TIMEOUT_SECS), response.text())
                        .await??;
                if let Some(cap) = crate::llm::metered::http_spend_cap(
                    ProviderKind::ChatGptPlan,
                    status.as_u16(),
                    &error_headers,
                    &error_body,
                )
                .await
                {
                    return Err(cap.into());
                }
                let plan_error =
                    ChatGptPlanError::from_http(status.as_u16(), &error_body, request_id);
                match http_failure_step(
                    true,
                    status.as_u16(),
                    plan_error.as_ref().map(|e| &e.failure),
                    refreshed,
                    attempt,
                ) {
                    HttpFailureStep::Refresh => {
                        bearer = tokens.after_rejection(&bearer).await?;
                        refreshed = true;
                        continue;
                    }
                    HttpFailureStep::Backoff => {
                        tokio::time::sleep(crate::llm::retry_delay(attempt, 1)).await;
                        continue;
                    }
                    HttpFailureStep::Fail => {}
                }
                if let Some(error) = plan_error {
                    self.observe_plan_failure(&error).await;
                    return Err(error.into());
                }
                return Err(crate::llm::ProviderError::from_http(
                    "ChatGPT plan",
                    status.as_u16(),
                    &error_body,
                )
                .into());
            }

            let parsed = read_search_stream(response.bytes_stream()).await;
            if let Err(error) = &parsed {
                if let Some(plan) = error.downcast_ref::<ChatGptPlanError>() {
                    self.observe_plan_failure(plan).await;
                }
            }
            let parsed = parsed?;
            return Ok(WebSearchResult {
                text: crate::llm::web_search::openai::parse_response(&parsed, max_results)?,
                usage: crate::llm::usage_wire::from_json(&parsed),
                model: model.to_string(),
                provider: Some(BillingProvider::ChatgptPlan),
            });
        }
    }
}

/// INVARIANT: Read the terminal response, which carries annotations absent from text deltas.
async fn read_search_stream<S, B>(mut stream: S) -> Result<Value, BoxError>
where
    S: Stream<Item = Result<B, reqwest::Error>> + Unpin,
    B: AsRef<[u8]>,
{
    let mut buffer = String::new();
    let mut carry = Vec::new();
    let mut event = String::new();
    while let Some(chunk) =
        tokio::time::timeout(Duration::from_secs(CHUNK_TIMEOUT_SECS), stream.next()).await?
    {
        crate::llm::push_utf8_chunk(&mut carry, chunk?.as_ref(), &mut buffer);
        while let Some(end) = buffer.find('\n') {
            let line = buffer[..end].trim_end_matches('\r').to_string();
            buffer.drain(..=end);
            if let Some(kind) = line.strip_prefix("event:") {
                event = kind.trim().to_string();
            } else if let Some(data) = line.strip_prefix("data:") {
                let data: Value = serde_json::from_str(data.trim())?;
                let kind = data["type"].as_str().unwrap_or(&event);
                match kind {
                    "response.completed" | "response.incomplete" => {
                        let response = data
                            .get("response")
                            .ok_or("ChatGPT plan search omitted the response")?;
                        // INVARIANT: Refusals must never become a successful empty search.
                        for item in response["output"]
                            .as_array()
                            .map(Vec::as_slice)
                            .unwrap_or(&[])
                        {
                            for part in item["content"].as_array().map(Vec::as_slice).unwrap_or(&[])
                            {
                                if part["type"] == "refusal" {
                                    return Err(crate::llm::ProviderError::from_stream_error("ChatGPT plan", &json!({
                                        "code": "content_policy_violation",
                                        "message": part["refusal"].as_str().unwrap_or("Web search refused"),
                                    })).into());
                                }
                            }
                        }
                        return Ok(response.clone());
                    }
                    "response.failed" | "error" => {
                        let error = data
                            .pointer("/response/error")
                            .or_else(|| data.get("error"))
                            .unwrap_or(&data);
                        if let Some(plan) = ChatGptPlanError::from_error_object(error) {
                            return Err(plan.into());
                        }
                        return Err(crate::llm::ProviderError::from_stream_error(
                            "ChatGPT plan",
                            error,
                        )
                        .into());
                    }
                    _ => {}
                }
            } else if line.is_empty() {
                event.clear();
            }
        }
    }
    Err("ChatGPT plan search stream ended before its terminal response".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fragmented_search_stream_preserves_terminal_citations_and_usage() {
        let terminal = json!({
            "type": "response.completed",
            "response": {
                "status": "completed",
                "output": [
                    {"type": "web_search_call", "id": "ws_1", "status": "completed",
                     "action": {"type": "search", "query": "news", "sources": [
                         {"type": "url", "url": "https://example.com/news"}
                     ]}},
                    {"type": "message", "role": "assistant", "status": "completed", "content": [
                        {"type": "output_text", "text": "News café", "annotations": [
                            {"type": "url_citation", "url": "https://example.com/news",
                             "title": "News", "start_index": 0, "end_index": 9},
                            {"type": "url_citation", "url": "https://example.com/news",
                             "title": "News", "start_index": 0, "end_index": 4}
                        ]}
                    ]}
                ],
                "usage": {"input_tokens": 12, "output_tokens": 4}
            }
        });
        let wire = format!(
            "data: {{\"type\":\"response.web_search_call.in_progress\",\"item_id\":\"ws_1\"}}\r\n\r\n\
             data: {{\"type\":\"response.web_search_call.completed\",\"item_id\":\"ws_1\"}}\r\n\r\n\
             data: {{\"type\":\"response.output_text.delta\",\"delta\":\"News café\"}}\r\n\r\n\
             data: {terminal}\r\n\r\ndata: [DONE]\r\n\r\n"
        );
        let stream = futures::stream::iter(wire.as_bytes().chunks(1).map(Ok::<_, reqwest::Error>));
        let response = read_search_stream(stream).await.unwrap();
        let text = crate::llm::web_search::openai::parse_response(&response, 2).unwrap();
        assert!(text.contains("News café"));
        assert!(text.contains("Sources:"));
        assert_eq!(text.matches("https://example.com/news").count(), 1);
        assert_eq!(
            crate::llm::usage_wire::from_json(&response)
                .unwrap()
                .input_tokens,
            12
        );
    }

    #[tokio::test(start_paused = true)]
    async fn progressing_search_can_run_longer_than_a_minute() {
        let chunks = [
            ": heartbeat\n\n",
            "event: response.completed\ndata: {\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n",
        ];
        let stream = futures::stream::iter(chunks).then(|chunk| async move {
            tokio::time::sleep(Duration::from_secs(45)).await;
            Ok::<_, reqwest::Error>(chunk.as_bytes())
        });
        let started = tokio::time::Instant::now();
        let response = read_search_stream(Box::pin(stream)).await.unwrap();
        assert_eq!(response["status"], "completed");
        assert_eq!(started.elapsed(), Duration::from_secs(90));
    }

    #[tokio::test(start_paused = true)]
    async fn stalled_search_times_out_without_waiting_forever() {
        let stream = futures::stream::pending::<Result<&[u8], reqwest::Error>>();
        let started = tokio::time::Instant::now();
        let error = read_search_stream(stream).await.unwrap_err();
        assert!(error.is::<tokio::time::error::Elapsed>());
        assert_eq!(started.elapsed(), Duration::from_secs(CHUNK_TIMEOUT_SECS));
    }
}
