//! LLM-facing schemas for web tools (web_search, fetch_news).

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn fetch_news_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::FETCH_NEWS.to_string(),
            description: "Fetch recent news articles on a topic. Uses GDELT global news database which covers news in all languages worldwide. Automatically prioritizes sources from user's country based on their timezone.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": "The news topic or search query (e.g., 'Trump', 'climate', 'sports', 'AI')"
                    },
                    "max_articles": {
                        "type": "integer",
                        "description": "Maximum number of articles to return (default: 5)"
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
            description: "Search the web for information. Use this when you need to look up facts, verify information, or find current data you're unsure about. Search once or twice max — if you found the answer, STOP searching.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "The search query (e.g., 'python list comprehension', 'best pizza recipe')"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 5)"
                    }
                },
                "required": ["query"]
            }),
        },
    ]
}
