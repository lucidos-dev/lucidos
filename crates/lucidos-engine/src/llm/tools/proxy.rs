//! LLM-facing schemas for the proxy / HTTP tools
//! (reload_proxy_modules, proxy_request, http_request).

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn proxy_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::RELOAD_PROXY_MODULES.to_string(),
            description: "Re-scan `data/auth-modules/` and reload every WASM signer module. Use after dropping a new or updated `<name>.wasm` (and optional `<name>.manifest.json` sidecar) into that directory — the new modules become available to the proxy pipeline immediately, without restarting the engine. Returns the list of modules now loaded so you can confirm what's available. Note: `install_plugin` auto-reloads when the plugin ships `auth-modules/` files; this tool is only needed for hand-placed modules.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: tn::PROXY_REQUEST.to_string(),
            description: "Call a backend configured in `data/config/apis.json` through the engine proxy. Prefer this over `http_request` whenever the API has a proxy entry — the credential is resolved by the engine and never appears in the tool args, the tool transcript, or any logs. Returns the raw response body for 2xx; for non-2xx returns `HTTP Error N: ...`. The proxy `name` indexes into `apis.json`; `path` is appended to the configured `base_url`. Auth pipeline supports static credentials (bearer/api_key/basic/query_param), HMAC signing, script-handshake login flows, and per-request WASM signer modules — the engine handles authentication transparently.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Proxy name as configured in `data/config/apis.json` (e.g., 'sonos', 'comfort')."
                    },
                    "path": {
                        "type": "string",
                        "description": "Path appended to the configured base_url (e.g., '/living-room/play'). Optional, defaults to root."
                    },
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE", "PATCH"],
                        "description": "HTTP method. Defaults to GET."
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional caller-supplied headers (Content-Type, Accept, …). The engine adds the configured auth header automatically.",
                        "additionalProperties": { "type": "string" }
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional request body for POST/PUT/PATCH."
                    }
                },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: tn::HTTP_REQUEST.to_string(),
            description: "Make an HTTP request to fetch data from an API. Use temp_path for raw data (.lucidos/tmp/, not git-tracked). Use output_path only for final artifacts (auto-committed).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE"],
                        "description": "HTTP method"
                    },
                    "url": {
                        "type": "string",
                        "description": "Full URL to request"
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional headers as key-value pairs",
                        "additionalProperties": { "type": "string" }
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional request body (for POST/PUT)"
                    },
                    "temp_path": {
                        "type": "string",
                        "description": "Filename to save in .lucidos/tmp/ (not git-tracked). Just the filename, e.g. 'google_doc.json' — NOT '.lucidos/tmp/google_doc.json'."
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Relative path under data/ to save response (e.g., artifacts/imported/oura/sleep.json). Git committed automatically. Refuses responses larger than 100 MB — for bulk reference data, fetch into temp_path or move the persisted file to ~/.lucidos/data/<name>/ (see system-knowhow/best-practices rule 8)."
                    }
                },
                "required": ["method", "url"]
            }),
        },
    ]
}
