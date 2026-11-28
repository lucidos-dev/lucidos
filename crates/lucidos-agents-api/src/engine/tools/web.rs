use super::super::LucidosEngine;
use std::time::Duration;

#[easy_ext::ext(WebExt)]
pub(crate) impl LucidosEngine {
    async fn execute_web_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "fetch_news" => {
                let topic = args["topic"].as_str().unwrap_or("");
                let max_articles = args["max_articles"].as_u64().unwrap_or(5) as usize;

                if topic.is_empty() {
                    return Ok("Error: topic is required".to_string());
                }

                let mut all_articles: Vec<String> = Vec::new();
                let client = match reqwest::Client::builder()
                    .timeout(Duration::from_secs(15))
                    .build()
                {
                    Ok(c) => c,
                    Err(e) => {
                        return Ok(format!(
                            "Error: failed to build HTTP client for fetch_news: {}",
                            e
                        ));
                    }
                };

                let gdelt_url = format!(
                    "https://api.gdeltproject.org/api/v2/doc/doc?query={}&mode=artlist&maxrecords={}&format=json&sort=datedesc",
                    urlencoding::encode(topic),
                    max_articles.saturating_add(5)
                );

                match client.get(&gdelt_url).send().await {
                    Ok(response) => {
                        let status = response.status();
                        if !status.is_success() {
                            return Ok(format!(
                                "Error: news search failed: GDELT returned HTTP {}",
                                status
                            ));
                        }
                        let body = match response.text().await {
                            Ok(body) => body,
                            Err(e) => {
                                return Ok(format!(
                                    "Error: news search failed reading the GDELT response: {}",
                                    e
                                ));
                            }
                        };
                        match parse_gdelt_articles(&body, max_articles) {
                            Ok(rendered) => all_articles.extend(rendered),
                            Err(msg) => return Ok(format!("Error: {}", msg)),
                        }
                    }
                    Err(e) => {
                        return Ok(format!("Error: news search request failed: {}", e));
                    }
                }

                if all_articles.is_empty() {
                    Ok(format!(
                        "No news articles found matching '{}'. Try a broader topic or different keywords.",
                        topic
                    ))
                } else {
                    let shown: Vec<String> = all_articles.into_iter().take(max_articles).collect();
                    let count = shown.len();
                    Ok(format!(
                        "Found {} articles about '{}':\n\n{}",
                        count,
                        topic,
                        shown.join("\n\n")
                    ))
                }
            }
            "web_search" => {
                let query = args["query"].as_str().unwrap_or("");
                let max_results = args
                    .get("max_results")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(5) as usize;

                if query.is_empty() {
                    return Ok("Error: query is required".to_string());
                }

                // INVARIANT: Resolved over the configured provider set — NOT over the chat
                // model's provider, so a user whose chat provider has no search
                // tool still searches via another configured one. The chain
                // itself owns backend selection, fallthrough, and the
                // nothing-configured message; see `llm::web_search`.
                let capture = crate::engine::AuxCapture::new(
                    &self.event_bus,
                    thread_id,
                    crate::engine::ContextPurpose::WebSearch,
                );
                let chain = self.current_web_search();
                match capture.search(&chain, query, max_results).await {
                    Ok(result) => Ok(result),
                    Err(e) => Ok(format!("Error: Search failed: {}", e)),
                }
            }
            _ => Err(format!("Unknown web tool: {}", name).into()),
        }
    }
}

fn parse_gdelt_articles(body: &str, max_articles: usize) -> Result<Vec<String>, String> {
    if body.trim().is_empty() {
        return Ok(Vec::new());
    }
    let json: serde_json::Value = serde_json::from_str(body)
        .map_err(|e| format!("news search failed parsing the GDELT response: {}", e))?;
    let Some(articles) = json.get("articles").and_then(|v| v.as_array()) else {
        return Err("news search failed: the GDELT response has no 'articles' list".to_string());
    };
    Ok(articles
        .iter()
        .take(max_articles)
        .map(render_article)
        .collect())
}

fn render_article(article: &serde_json::Value) -> String {
    let title = article["title"].as_str().unwrap_or("Untitled");
    let url = article["url"].as_str().unwrap_or("");
    let source = article["domain"].as_str().unwrap_or("Unknown");
    let date = article["seendate"].as_str().unwrap_or("");
    format!(
        "**[{}]** {} ({})\n  {}",
        source,
        title,
        gdelt_date(date),
        url
    )
}

fn gdelt_date(seendate: &str) -> String {
    let head: Vec<char> = seendate.chars().take(8).collect();
    if head.len() < 8 {
        return seendate.to_string();
    }
    format!(
        "{}-{}-{}",
        head[..4].iter().collect::<String>(),
        head[4..6].iter().collect::<String>(),
        head[6..8].iter().collect::<String>()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_body_with_no_articles_list_is_an_error_not_an_empty_result() {
        for body in [
            r#"{"status":"rate limited"}"#,
            r#"{"articles":"oops"}"#,
            r#"[]"#,
        ] {
            let err = parse_gdelt_articles(body, 5)
                .expect_err("a body carrying no articles list must not read as no news");
            assert!(err.contains("'articles' list"), "got: {err}");
        }
    }

    #[test]
    fn an_empty_body_is_a_genuine_no_results_answer() {
        assert_eq!(parse_gdelt_articles("", 5), Ok(Vec::new()));
        assert_eq!(parse_gdelt_articles("   \n", 5), Ok(Vec::new()));
    }

    #[test]
    fn an_unparseable_body_names_the_parse_failure() {
        let err = parse_gdelt_articles("Your query was too short", 5).unwrap_err();
        assert!(err.contains("parsing the GDELT response"), "got: {err}");
    }

    #[test]
    fn articles_render_with_source_title_date_and_url() {
        let body = r#"{"articles":[
            {"title":"A","url":"https://example.com/a","domain":"example.com",
             "seendate":"20260907T101500Z"},
            {"title":"B","url":"https://example.org/b","domain":"example.org",
             "seendate":"20260906T090000Z"}
        ]}"#;
        let rendered = parse_gdelt_articles(body, 5).expect("a well-formed body renders");
        assert_eq!(rendered.len(), 2);
        assert_eq!(
            rendered[0],
            "**[example.com]** A (2026-09-07)\n  https://example.com/a"
        );
        assert!(rendered[1].contains("2026-09-06"), "got: {}", rendered[1]);
    }

    #[test]
    fn the_article_cap_is_applied_to_the_parsed_list() {
        let body = r#"{"articles":[{"title":"A"},{"title":"B"},{"title":"C"}]}"#;
        assert_eq!(parse_gdelt_articles(body, 2).unwrap().len(), 2);
        assert_eq!(parse_gdelt_articles(body, 0).unwrap().len(), 0);
    }

    #[test]
    fn a_sparse_article_falls_back_to_named_placeholders() {
        let rendered = render_article(&serde_json::json!({}));
        assert!(rendered.contains("Untitled"), "got: {rendered}");
        assert!(rendered.contains("Unknown"), "got: {rendered}");
    }

    #[test]
    fn a_seendate_too_short_to_hold_a_day_is_passed_through() {
        assert_eq!(gdelt_date(""), "");
        assert_eq!(gdelt_date("2026"), "2026");
        assert_eq!(gdelt_date("20260907"), "2026-09-07");
    }
}
