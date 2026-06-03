//! LLM-facing schemas for app-management tools (create/list/refresh/capture,
//! load_knowhow, refresh_file).


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn app_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::CREATE_APP.to_string(),
            description: "Create a new app with a UI. The app will be saved to data/apps/{id}/ with manifest.json and index.html. IMPORTANT: App data should be stored in data/artifacts/ (e.g., data/artifacts/habits/data.json), NOT in data/apps/. The manifest.json MUST include both 'name' and 'description' fields.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "App ID (folder name, lowercase with hyphens, e.g., 'habit-tracker')"
                    },
                    "name": {
                        "type": "string",
                        "description": "Display name for the app"
                    },
                    "description": {
                        "type": "string",
                        "description": "One-line description"
                    },
                    "html_content": {
                        "type": "string",
                        "description": "Initial HTML content for the app's index.html"
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
            description: "Load the full content of a know-how document. The system prompt lists available know-how by name and description — use this tool to load the full content when a know-how file is relevant to the user's request.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Know-how ID as shown in the know-how list (e.g., 'lucidos/cross-workspace')"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::REFRESH_FILE.to_string(),
            description: "Refresh the user's file preview window to show updated content. Call this after writing/editing a file that the user has open.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path to refresh (e.g., artifacts/notes.md)"
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::REFRESH_APP.to_string(),
            description: "Refresh the iframe of the currently-open app so it reflects on-disk changes, then return a screenshot and DOM snapshot (unless skip_capture is true). If the app isn't currently open, the refresh is a no-op and the capture step will fail — use navigate_ui first when you need to look at an app the user hasn't opened.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "app_id": {
                        "type": "string",
                        "description": "The app ID to refresh"
                    },
                    "skip_capture": {
                        "type": "boolean",
                        "description": "If true, skip the automatic screenshot capture (useful during rapid iteration). Default: false"
                    }
                },
                "required": ["app_id"]
            }),
        },
        ToolDefinition {
            name: tn::CAPTURE_APP.to_string(),
            description: "Capture a screenshot and DOM snapshot of the currently open app UI to see what it looks like. Use this to check the visual result of your changes or when the user asks you to look at the UI.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "app_id": {
                        "type": "string",
                        "description": "The app ID whose UI to capture"
                    }
                },
                "required": ["app_id"]
            }),
        },
    ]
}
