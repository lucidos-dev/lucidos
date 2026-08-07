//! LLM-facing schemas for the proxy / HTTP tools
//! (reload_proxy_modules, proxy_request, http_request).

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn proxy_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::RELOAD_PROXY_MODULES.to_string(),
            description: "Re-scan `data/auth-modules/` and reload every WASM signer module, returning the list now loaded. For a hand-placed `<name>.wasm` plus optional `<name>.manifest.json` sidecar; it reaches the proxy pipeline with no engine restart. Installing a plugin that ships auth-modules/ reloads them for you.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
        },
        ToolDefinition {
            name: tn::PROXY_REQUEST.to_string(),
            description: "Call a backend configured in `data/config/apis.json` through the engine proxy. Prefer it over `http_request` whenever the API has a proxy entry: the engine resolves the credential, so it never appears in the tool args, the transcript, or any log, and it handles HMAC signing, script-handshake logins and WASM signers transparently. Returns the raw body for 2xx, or `HTTP Error N: ...`.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Proxy name from `data/config/apis.json` (e.g. 'sonos')."
                    },
                    "path": {
                        "type": "string",
                        "description": "Path appended to the configured base_url (e.g. '/living-room/play'). Defaults to root."
                    },
                    "method": {
                        "type": "string",
                        "enum": ["GET", "POST", "PUT", "DELETE", "PATCH"],
                        "description": "HTTP method. Defaults to GET."
                    },
                    "headers": {
                        "type": "object",
                        "description": "Optional headers; the engine adds the auth header itself.",
                        "additionalProperties": { "type": "string" }
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional body for POST/PUT/PATCH."
                    }
                },
                "required": ["name"]
            }),
        },
        ToolDefinition {
            name: tn::HTTP_REQUEST.to_string(),
            description: "Make an HTTP request to an API. temp_path saves raw data to .lucidos/tmp/ (not git-tracked); output_path is for a final artifact and is auto-committed.".to_string(),
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
                        "description": "Optional headers as key-value pairs.",
                        "additionalProperties": { "type": "string" }
                    },
                    "body": {
                        "type": "string",
                        "description": "Optional body for POST/PUT."
                    },
                    "temp_path": {
                        "type": "string",
                        "description": "Bare filename to save in .lucidos/tmp/ (not git-tracked), e.g. 'google_doc.json'."
                    },
                    "output_path": {
                        "type": "string",
                        "description": "Relative path under data/ to save the response, committed automatically. Refuses over 100 MB: use temp_path, or move the file to ~/.lucidos/data/<name>/."
                    }
                },
                "required": ["method", "url"]
            }),
        },
    ]
}
