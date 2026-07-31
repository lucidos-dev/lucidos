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
                    max_articles + 5 // Fetch a few extra in case of duplicates
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
                        // GDELT returns an empty/whitespace body for a genuine
                        // no-results query; only a non-empty body that won't parse
                        // signals an upstream problem worth surfacing (vs. silently
                        // reporting "no news", which masks the failure from the LLM).
                        if !body.trim().is_empty() {
                            let json = match serde_json::from_str::<serde_json::Value>(&body) {
                                Ok(json) => json,
                                Err(e) => {
                                    return Ok(format!(
                                        "Error: news search failed parsing the GDELT response: {}",
                                        e
                                    ));
                                }
                            };
                            if let Some(articles) = json["articles"].as_array() {
                                for article in articles.iter().take(max_articles) {
                                    let title = article["title"].as_str().unwrap_or("Untitled");
                                    let url = article["url"].as_str().unwrap_or("");
                                    let source = article["domain"].as_str().unwrap_or("Unknown");
                                    let date = article["seendate"].as_str().unwrap_or("");
                                    let date_formatted = if date.chars().count() >= 8 {
                                        let chars: Vec<char> = date.chars().take(8).collect();
                                        let yyyy: String = chars[..4].iter().collect();
                                        let mm: String = chars[4..6].iter().collect();
                                        let dd: String = chars[6..8].iter().collect();
                                        format!("{}-{}-{}", yyyy, mm, dd)
                                    } else {
                                        date.to_string()
                                    };
                                    all_articles.push(format!(
                                        "**[{}]** {} ({})\n  {}",
                                        source, title, date_formatted, url
                                    ));
                                }
                            }
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
                    let result = all_articles
                        .into_iter()
                        .take(max_articles)
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    Ok(format!(
                        "Found {} articles about '{}':\n\n{}",
                        result.matches("**[").count(),
                        topic,
                        result
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
            _ => Ok(format!("Unknown web tool: {}", name)),
        }
    }
}
