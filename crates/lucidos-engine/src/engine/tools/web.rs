use super::super::LucidosEngine;
use crate::llm::WebSearchProvider;
use std::time::Duration;

impl LucidosEngine {
    pub(crate) async fn execute_web_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
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
                    // Saturating: the arg carries no cap, and a plain `+ 5` on
                    // a model-supplied `usize::MAX` wraps to 4 in release.
                    max_articles.saturating_add(5) // A few extra, in case of duplicates
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
                    // Counted, not re-derived from the rendered text: a title
                    // carrying the `**[` marker inflated the reported total.
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

                // Resolved over the configured provider set — NOT over the chat
                // model's provider, so a user whose chat provider has no search
                // tool still searches via another configured one. The chain
                // itself owns backend selection, fallthrough, and the
                // nothing-configured message; see `llm::web_search`.
                match self.current_web_search().search(query, max_results).await {
                    Ok(result) => Ok(result),
                    Err(e) => Ok(format!("Error: Search failed: {}", e)),
                }
            }
            _ => Err(format!("Unknown web tool: {}", name).into()),
        }
    }
}

/// Render the articles a `fetch_news` response body carries, newest first.
///
/// An empty or whitespace body is the genuine no-results answer GDELT gives a
/// query nothing matched, so it yields an empty list. Every other body must
/// parse AND carry an `articles` list.
///
/// A body that parsed into some other shape is an upstream problem, so it is an
/// error here rather than an empty list. An empty list reaches the model as "no
/// news on this topic". It then states that to the user as a fact about the
/// world, when it is a fact about the search.
///
/// The error text carries no `Error:` prefix. The caller adds it, so the tool's
/// failure shape stays stated in one place.
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

/// One article as the tool result shows it: source, title, date and URL.
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

/// The `YYYYMMDD` head of a GDELT `seendate`, rendered with dashes. A value too
/// short to hold one is passed through: showing the raw form beats inventing a
/// day.
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

    /// THE regression. A body that parsed into another shape used to fall
    /// through to the tool's "No news articles found" line. The model then told
    /// the user nothing had been published on their topic.
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

    /// The one body that really does mean no results. GDELT answers a query
    /// nothing matched with nothing at all.
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
