use super::super::LucidosEngine;
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
                        if let Ok(body) = response.text().await {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
                                if let Some(articles) = json["articles"].as_array() {
                                    for article in articles.iter().take(max_articles) {
                                        let title = article["title"].as_str().unwrap_or("Untitled");
                                        let url = article["url"].as_str().unwrap_or("");
                                        let source =
                                            article["domain"].as_str().unwrap_or("Unknown");
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
                    }
                    Err(e) => {
                        log!(@fetch_news, "GDELT request failed: {}", e);
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

                // Use Gemini Google Search grounding via Vertex AI
                match &self.extractor {
                    Some(extractor) => {
                        match extractor
                            .provider()
                            .search_with_grounding(query, max_results)
                            .await
                        {
                            Ok(result) => Ok(result),
                            Err(e) => Ok(format!("Error: Search failed: {}", e)),
                        }
                    }
                    None => Ok(
                        "Error: web_search requires Vertex AI configuration (VERTEX_PROJECT_ID)"
                            .to_string(),
                    ),
                }
            }
            _ => Ok(format!("Unknown web tool: {}", name)),
        }
    }
}
