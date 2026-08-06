//! LLM-facing schemas for app-management tools (create/list/refresh/capture,
//! load_knowhow).

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
                        "description": "Initial HTML content for the app's index.html. By default the app must inherit the Lucidos theme so it follows the user's light/dark (OS) appearance: in <head> include <script src=\"/api/v1/sdk-prefs.js\"></script>, <link rel=\"stylesheet\" href=\"/api/v1/sdk-iframe.css\">, and <script src=\"/api/v1/sdk.js\"></script>; call lucidos.ui.applyPreferences() and lucidos.ui.watchPreferences() on load; and style with the theme CSS variables (var(--bg-primary), var(--text-primary), var(--accent), var(--border-color), …) instead of hardcoded colors. For buttons, lists, badges, progress, and markdown, REUSE Lucidos's shared component classes from sdk-iframe.css so the app matches the host shell exactly: primary buttons are <button class=\"action-btn\"> (with .action-btn-confirm / .action-btn-danger variants, and .action-btn-secondary for a neutral secondary button), icon buttons .icon-btn, plus .label, .title, .list-row*, .segmented-control, .markdown-content, .progress-bar, .empty-state — a plain unclassed <button> does NOT match Lucidos's blue primary button, and do NOT hand-roll an outlined secondary button (use .action-btn-secondary). SIZE EVERYTHING IN rem, NOT px: the user's font-size/UI-scale preference is applied as the root font-size, so only rem/em units scale with it — a px-sized app ignores the user's font size. Use rem for text/spacing/radii (or the --space-*/--radius-* tokens); 1px borders are the only acceptable px. See the building-an-app / js-sdk knowhow for the full scaffold, variable list, token values, and component-class table. Opt out only for apps that ship their own complete visual identity (game, chart canvas, embedded third-party UI)."
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
                        "description": "Know-how ID as shown in the know-how list (e.g., 'system-knowhow/best-practices')"
                    }
                },
                "required": ["id"]
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
