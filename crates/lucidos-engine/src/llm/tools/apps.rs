//! LLM-facing schemas for app-management tools (create/list/refresh/capture,
//! load_knowhow).

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn app_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::CREATE_APP.to_string(),
            description: "Create a new app with a UI, saved to data/apps/<id>/ with manifest.json and index.html. The manifest MUST carry both 'name' and 'description'. App DATA belongs in data/artifacts/ (e.g. artifacts/habits/data.json), never in data/apps/.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Folder name, lowercase with hyphens (e.g. 'habit-tracker')."
                    },
                    "name": {
                        "type": "string",
                        "description": "Display name."
                    },
                    "description": {
                        "type": "string",
                        "description": "One-line description."
                    },
                    "html_content": {
                        "type": "string",
                        "description": "Initial HTML for index.html. Make it native to Lucidos: inherit the theme, theme CSS vars NOT hardcoded colors, shared component classes NOT bare elements, rem NOT px, and a fluid layout holding at PHONE WIDTH (one column first, no fixed px width, nothing top-right, where the host draws its fullscreen exit). load_knowhow('system-knowhow/building-an-app') for the scaffold, tokens, components and responsive rules. Opt out only for an app with its own complete visual identity."
                    }
                },
                "required": ["id", "name", "description", "html_content"]
            }),
        },
        ToolDefinition {
            name: tn::LIST_APPS.to_string(),
            description: "List all available apps in the workspace.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::LOAD_KNOWHOW.to_string(),
            description: "Load the full content of a knowhow document. The system prompt lists the available ones by name and description; call this when one is relevant.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Knowhow id as shown in the knowhow list (e.g. 'system-knowhow/best-practices')"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::REFRESH_APP.to_string(),
            description: "Refresh the currently-open app UI so it reflects on-disk changes, then return a screenshot and DOM snapshot unless skip_capture is true. If the app is not open the refresh is a no-op and the capture fails, so navigate_ui to it first.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "app_id": {
                        "type": "string",
                        "description": "App id to refresh."
                    },
                    "skip_capture": {
                        "type": "boolean",
                        "description": "Skip the screenshot capture, for rapid iteration. Default false."
                    }
                },
                "required": ["app_id"]
            }),
        },
        ToolDefinition {
            name: tn::CAPTURE_APP.to_string(),
            description: "Screenshot and DOM snapshot of the currently open app UI, to check the visual result of your changes or when the user asks you to look.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "app_id": {
                        "type": "string",
                        "description": "App id to capture."
                    }
                },
                "required": ["app_id"]
            }),
        },
    ]
}
