//! LLM-facing schemas for the autonomous browser-driving tools.
//!
//! The matching handlers live in `engine::tools::browser` and the runtime
//! state lives in `runtime::browser`. This module owns only the JSON-shape
//! contract that gets advertised to the LLM.
//!
//! First family extracted as part of the `llm/tools.rs` split — see the
//! top-of-`mod.rs` plan for the remaining families.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// All browser-driving tool definitions, in display order. Spliced into
/// `get_default_tools()` so the LLM still sees the whole tool surface in
/// one vec.
pub(super) fn browser_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::BROWSER_OPEN.to_string(),
            description: "Open a web page in a browser session. Returns the page text content. The browser uses a persistent profile — logins, cookies, and localStorage carry over between sessions.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Full URL to navigate to (e.g., 'https://news.ycombinator.com')"
                    },
                    "wait_for": {
                        "type": "string",
                        "description": "Optional CSS selector to wait for before returning content"
                    },
                    "visible": {
                        "type": "boolean",
                        "description": "Open a visible Chrome window the user can see and interact with. Use when the user says 'show me', 'let me log in', or when a site blocks headless browsers. Default: false (headless)."
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_EXTRACT.to_string(),
            description: "Extract content from elements on the current page. Use after browser_open.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for elements to extract (e.g., '.story-title', 'table.data', 'a.link')"
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "html", "links", "table"],
                        "description": "Output format: 'text' (innerText), 'html' (outerHTML), 'links' (URLs with text), 'table' (table rows as pipe-separated)"
                    }
                },
                "required": ["selector", "format"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_CLICK.to_string(),
            description: "Click an element on the current page. Use for buttons, links, or interactive elements.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the element to click"
                    },
                    "wait_navigation": {
                        "type": "boolean",
                        "description": "Wait for page navigation after click (default: false)"
                    }
                },
                "required": ["selector"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_TYPE.to_string(),
            description: "Type text into an input field on the current page. Use for search boxes, forms, etc.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the input element"
                    },
                    "text": {
                        "type": "string",
                        "description": "Text to type into the input"
                    },
                    "clear": {
                        "type": "boolean",
                        "description": "Clear existing content before typing (default: false)"
                    },
                    "enter": {
                        "type": "boolean",
                        "description": "Press Enter after typing (default: false)"
                    }
                },
                "required": ["selector", "text"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_EVAL.to_string(),
            description: "Execute JavaScript code on the current page and return the result. Use for complex interactions or data extraction.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "JavaScript code to execute. Return value will be converted to string/JSON."
                    }
                },
                "required": ["script"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_SCREENSHOT.to_string(),
            description: "Take a screenshot and save it to artifacts. Can optionally navigate to a URL first.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ to save the screenshot (e.g., 'artifacts/screenshots/page.png')"
                    },
                    "url": {
                        "type": "string",
                        "description": "Optional URL to navigate to before taking the screenshot. Use this when taking screenshots of multiple different sites."
                    },
                    "selector": {
                        "type": "string",
                        "description": "Optional CSS selector to screenshot a specific element"
                    },
                    "full_page": {
                        "type": "boolean",
                        "description": "Capture the full scrollable page instead of just the viewport (default: false)"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_CLOSE.to_string(),
            description: "Close the browser session. The browser will also auto-close after 30 minutes of inactivity.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_FORGET_LOGIN.to_string(),
            description: "Remove a recorded browser login (e.g., expired session, user logged out).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "domain": {
                        "type": "string",
                        "description": "Domain to forget (e.g., 'github.com')"
                    }
                },
                "required": ["domain"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_CLEAR_DATA.to_string(),
            description: "Delete all Lucidos browser data: cookies, logins, localStorage, cache. Closes any running browser first. Use when the user wants to start fresh.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
    ]
}
