//! LLM-facing schemas for the autonomous browser-driving tools.
//!
//! The matching handlers live in `engine::tools::browser` and the runtime
//! state lives in `runtime::browser`. This module owns only the JSON-shape
//! contract that gets advertised to the LLM.
//!
//! One of the per-domain families `get_default_tools()` splices together; the
//! full list lives in this module's parent (`llm::tools`).

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
            description: "Open a web page in a browser session and return its text. The profile is persistent, so logins, cookies and localStorage carry over between sessions.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Full URL to navigate to."
                    },
                    "wait_for": {
                        "type": "string",
                        "description": "Optional CSS selector to wait for before returning content."
                    },
                    "visible": {
                        "type": "boolean",
                        "description": "Open a visible window the user can see and interact with. Default false."
                    }
                },
                "required": ["url"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_EXTRACT.to_string(),
            description: "Extract content from elements on the current page, after browser_open.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the elements to extract."
                    },
                    "format": {
                        "type": "string",
                        "enum": ["text", "html", "links", "table"],
                        "description": "'text' innerText, 'html' outerHTML, 'links' URLs with their text, 'table' pipe-separated rows."
                    }
                },
                "required": ["selector", "format"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_CLICK.to_string(),
            description: "Click an element on the current page.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the element to click."
                    },
                    "wait_navigation": {
                        "type": "boolean",
                        "description": "Wait for navigation after the click (default false)."
                    }
                },
                "required": ["selector"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_TYPE.to_string(),
            description: "Type text into an input on the current page.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "selector": {
                        "type": "string",
                        "description": "CSS selector for the input."
                    },
                    "text": {
                        "type": "string",
                        "description": "Text to type."
                    },
                    "clear": {
                        "type": "boolean",
                        "description": "Clear existing content first (default false)."
                    },
                    "enter": {
                        "type": "boolean",
                        "description": "Press Enter after typing (default false)."
                    }
                },
                "required": ["selector", "text"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_EVAL.to_string(),
            description: "Run JavaScript on the current page and return the result.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "script": {
                        "type": "string",
                        "description": "JavaScript to execute; the return value is converted to string or JSON."
                    }
                },
                "required": ["script"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_SCREENSHOT.to_string(),
            description: "Screenshot the page into artifacts, optionally navigating to a URL first.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ to save it (e.g. 'artifacts/screenshots/page.png')."
                    },
                    "url": {
                        "type": "string",
                        "description": "Optional URL to navigate to first."
                    },
                    "selector": {
                        "type": "string",
                        "description": "Optional CSS selector to shoot one element."
                    },
                    "full_page": {
                        "type": "boolean",
                        "description": "Capture the full scrollable page, not just the viewport (default false)."
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_CLOSE.to_string(),
            description: "Close the browser session. It also auto-closes after 30 minutes idle.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_FORGET_LOGIN.to_string(),
            description: "Remove a recorded browser login (expired session, user logged out).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "domain": {
                        "type": "string",
                        "description": "Domain to forget (e.g. 'github.com')."
                    }
                },
                "required": ["domain"]
            }),
        },
        ToolDefinition {
            name: tn::BROWSER_CLEAR_DATA.to_string(),
            description: "Delete all Lucidos browser data (cookies, logins, localStorage, cache), closing any running browser first.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
    ]
}
