//! LLM-facing schemas for workspace file tools (read/write/edit, search,
//! import). Handlers live in `engine::tools::files`.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn read_write_edit_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::READ_FILE.to_string(),
            description: "Read a file under data/ or the ephemeral .lucidos/tmp/ scratch tree. Text and images both work; an SVG comes back as text, a large image is downsampled, and only files over 25 MB are rejected. Text over 50KB comes back in chunks ending with the exact `offset=` for the next call, and `start_line` plus `line_count` reads part of a long file. Reads inside .zip and .lucidos-plugin archives: point `path` past the archive segment.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g. artifacts/notes.md), or a scratch path under .lucidos/tmp/ exactly as http_request and git_clone report it back. May traverse a .zip or .lucidos-plugin segment to read an entry inside."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Byte offset to start from (default 0), from the previous truncated response. Text only, ignored when `start_line` is set."
                    },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line to start from. With `line_count`, reads a range. Text only."
                    },
                    "line_count": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Lines to read from `start_line` (default: to the end)."
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::WRITE_FILE.to_string(),
            description: "Create or update a file. For NEW files and FULL rewrites only: NEVER use it where edit_file would do, since rewriting introduces subtle regressions. Under data/ only, because it git-commits what it writes and so refuses the gitignored .lucidos/tmp/ scratch tree; create scratch with run_python.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g. artifacts/notes.md, apps/my-app/knowhow/api-reference.md). A knowhow/ path that exists in shared (~/.lucidos/knowhow/) but not locally updates the shared copy."
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write."
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message describing the intent."
                    }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: tn::EDIT_FILE.to_string(),
            description: "Targeted edit to an existing file, in one of two modes. Text mode is old_string plus new_string. JSON mode is json_path plus new_value, which handles parsing and re-serialization for a .json or .slides file, avoiding escaping and matching issues; prefer it there. Under data/ only: .lucidos/tmp/ is readable but not editable here, so use run_python.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g. artifacts/notes.md, apps/my-app/knowhow/api-reference.md). A knowhow/ path that exists in shared (~/.lucidos/knowhow/) but not locally updates the shared copy."
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Text mode: the exact text to find."
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Text mode: the replacement, different from old_string."
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Text mode: replace all occurrences, not just the first (default false)."
                    },
                    "json_path": {
                        "type": "string",
                        "description": "JSON mode: path to the target value. Dot notation, array indices (`sections[1]`), quoted keys (`dailyLog[\"2026-05-04\"]`), the JSONPath root (`$.streak`) and JSON Pointers (`/sections/1/title`) all work and mix freely."
                    },
                    "new_value": {
                        "description": "JSON mode: the replacement value, any JSON type."
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message describing the intent."
                    }
                },
                "required": ["path"]
            }),
        },
    ]
}

pub(super) fn search_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::LIST_FILES.to_string(),
            description: "List all files in the workspace (artifacts and app UIs). Call ONCE if needed, then use the results.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::GLOB_FILES.to_string(),
            description: "Find files by glob across artifacts/, apps/, knowhow/ and triggers/, relative to data/ (e.g. 'apps/**/index.html').".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern relative to data/."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max paths to return (default 200, max 1000). Sorted; `truncated: true` means the cap was hit."
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: tn::GREP_FILES.to_string(),
            description: "Regex-search file contents (Rust regex crate syntax) across artifacts/, apps/, knowhow/ and triggers/, skipping binaries and respecting workspace ignore rules. A line over 300 chars is truncated and the total capped at ~50 KB; narrow with `path_glob` when you hit `truncated: true`.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex, Rust regex crate syntax. Case-sensitive by default."
                    },
                    "path_glob": {
                        "type": "string",
                        "description": "Optional glob restricting which files are searched (e.g. 'apps/**/*.html'). Defaults to all files under data/."
                    },
                    "case_insensitive": {
                        "type": "boolean",
                        "description": "Match case-insensitively (default false)."
                    },
                    "max_matches": {
                        "type": "integer",
                        "description": "Total match cap (default 100, max 500). `truncated: true` means the cap was hit."
                    },
                    "context_lines": {
                        "type": "integer",
                        "description": "Lines of context before and after each match (default 0, max 5)."
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: tn::COPY_FILE.to_string(),
            description: "Copy a file server-side, without passing its content through the conversation. Use it instead of read_file plus write_file, and to promote a file out of scratch into artifacts/imported/<name>/ after a git_clone.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Source path under data/ or under .lucidos/tmp/."
                    },
                    "destination": {
                        "type": "string",
                        "description": "Destination path under data/. Git-committed, so .lucidos/tmp/ is not valid."
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message."
                    }
                },
                "required": ["source", "destination"]
            }),
        },
        ToolDefinition {
            name: tn::DELETE_FILE.to_string(),
            description: "Delete a file (recoverable from git history). Refuses a plugin-owned path and names the owning plugin id; use the plugins tool's uninstall action with that id instead, so the user sees a confirm panel first.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/. Tracked files only; remove .lucidos/tmp/ scratch with run_bash."
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message."
                    }
                },
                "required": ["path"]
            }),
        },
    ]
}

pub(super) fn import_file_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::IMPORT_FILE.to_string(),
            description: "Import a file from the local filesystem into artifacts/, committed to git and indexed for search. Refuses over 100 MB: move bulk reference data to ~/.lucidos/data/<name>/ and pin the absolute path in the relevant app's knowhow.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source_path": {
                        "type": "string",
                        "description": "Absolute path to the source file."
                    },
                    "destination": {
                        "type": "string",
                        "description": "Relative path in artifacts/imported/ (defaults to the original filename)."
                    }
                },
                "required": ["source_path"]
            }),
        },
    ]
}
