//! LLM-facing schemas for web tools (web_search, fetch_news).

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn fetch_news_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::FETCH_NEWS.to_string(),
            description: "Fetch recent news articles on a topic from the GDELT global news database, all languages. Sources from the user's own country, derived from their timezone, are prioritized automatically.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "The news topic or search query."
                    },
                    "max_articles": {
                        "type": "integer",
                        "description": "Max articles to return (default 5)."
                    }
                },
                "required": ["topic"]
            }),
        },
    ]
}

pub(super) fn web_search_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::WEB_SEARCH.to_string(),
            description: "Search the web to look up a fact, verify something, or find current data you are unsure of. Twice at most: once you have the answer, STOP searching.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Max results to return (default 5)."
                    }
                },
                "required": ["query"]
            }),
        },
    ]
}
