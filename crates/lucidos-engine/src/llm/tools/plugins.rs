//! LLM-facing schemas for plugin tools (install/marketplaces/check/update/uninstall).
//! Handlers live in `engine::tools::plugins`.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn plugin_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::INSTALL_PLUGIN.to_string(),
            description: "Stage a Lucidos plugin install for the user to confirm in a panel. A plugin is a coherent bundle of workspace content (apps, knowhow, triggers, scripts) that another author shipped. Source detection: a GitHub tree URL like 'https://github.com/owner/repo/tree/branch/subpath' is a monorepo install (clones the repo at that branch, uses subpath as the plugin root). A plain git URL or .git URL is cloned at the default branch. A path ending in '.lucidos-plugin' is unpacked locally. The user sees a confirm panel with the file list, source, and any overwrites — do NOT chat-ask the user about overwrites or repeat what the panel will show. After this call, do NOT respond about the install or claim it succeeded; the panel resolves it on Confirm/Cancel and emits the appropriate event. The next user message will tell you whether the install happened.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "GitHub tree URL (e.g., 'https://github.com/lucidos-dev/plugins/tree/main/browser-learning'), a plain git URL, or an absolute path to a .lucidos-plugin file."
                    }
                },
                "required": ["source"]
            }),
        },
        ToolDefinition {
            name: tn::REGISTER_PLUGIN_MARKETPLACE.to_string(),
            description: "Register or rename an App Store marketplace. A marketplace is a git repository or GitHub tree URL that the App Store scans for plugin manifests. Registration writes data/config/plugin-marketplaces.json, commits it, and kicks off a marketplace scan/auto-update pass. Use this when the user asks to add a plugin repo, marketplace, plugin marketplace, or app-store source. Do not use this to install a specific plugin; after registering, either navigate to the App Store or use install_plugin with a specific plugin source.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Git repository URL or GitHub tree URL to register as a marketplace."
                    },
                    "name": {
                        "type": "string",
                        "description": "Optional display name. Omit to derive a name from the repository."
                    }
                },
                "required": ["source"]
            }),
        },
        ToolDefinition {
            name: tn::CHECK_PLUGIN_UPDATES.to_string(),
            description: "Check installed plugins for newer versions at their `source` URL. With no `id`, surveys all currently-installed plugins. Network failures per plugin are reported as `error` entries — they don't abort the whole check. Returns JSON describing each plugin's installed_version, latest_version, and whether it changed.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Optional plugin id (e.g. 'browser-learning'). Omit to check every installed plugin."
                    }
                }
            }),
        },
        ToolDefinition {
            name: tn::UPDATE_PLUGIN.to_string(),
            description: "Apply the update for one installed plugin. Re-fetches the manifest from the recorded source, compares semver, and re-installs (with overwrite=true) if newer. Returns 'Already at latest (vX)' as a no-op when versions match.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The plugin id to update (e.g. 'browser-learning')."
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::UNINSTALL_PLUGIN.to_string(),
            description: "Stage a Lucidos plugin uninstall for the user to confirm in a panel. Resolves `id` against the plugin id, the manifest name, OR an `apps/<dir>` folder name installed by the plugin (case-insensitive, dash/underscore/whitespace-insensitive — \"No role playing\", \"no-role-playing\", and \"anti-sycophancy-critique\" all resolve to the same plugin). On exact match, looks up the install record, partitions the recorded files into still-on-disk vs already-missing, and shows the user a confirm panel listing what will be deleted. On multiple matches, returns an error listing the candidates so you can re-call with the exact id. Do NOT chat-ask which files to delete — the panel surfaces them. Do NOT manually delete_file the listed paths; the panel's Confirm button removes them atomically + reloads WASM signers if needed (and delete_file refuses plugin-owned paths anyway). After this call, do NOT respond about the uninstall or claim files were removed; the panel resolves it on Confirm/Cancel and emits the appropriate event. The next user message will tell you the outcome.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Plugin id, manifest name, or app folder installed by the plugin (e.g. 'browser-learning', 'Browser Learning', or 'browser-learning' app dir). Case- and dash/underscore/whitespace-insensitive."
                    }
                },
                "required": ["id"]
            }),
        },
    ]
}
