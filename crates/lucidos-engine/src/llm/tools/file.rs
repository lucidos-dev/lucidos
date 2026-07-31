//! LLM-facing schemas for workspace file tools (read/write/edit, search,
//! import). Handlers live in `engine::tools::files`.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn read_write_edit_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::READ_FILE.to_string(),
            description: "Read the contents of a file in the workspace. Supports text files and images (.png, .jpg, .jpeg, .gif, .webp — displayed visually). SVGs are returned as text. Large images (e.g. iPhone photos) are automatically downsampled to fit the model; only files over 25 MB are rejected. Text files >50KB are returned in chunks: the response ends with the exact `offset=` to pass on the next call to continue reading. Don't re-read content you've already seen.\n\nReads inside .zip and .lucidos-plugin archives transparently — point `path` past the archive segment, e.g. `artifacts/plugins/foo.lucidos-plugin/apps/x/index.html`. To inspect a small section of a long file use `start_line` + `line_count` instead of pulling the whole thing or shelling out to run_python.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g., artifacts/notes.md, apps/my-app/ui/main/index.html). May traverse a .zip or .lucidos-plugin segment to read an entry inside the archive (e.g. artifacts/plugins/foo.lucidos-plugin/apps/x/index.html)."
                    },
                    "offset": {
                        "type": "integer",
                        "description": "Byte offset to start reading from (default 0). Use the `offset=` value from the previous truncated response to read the next chunk. Text files only. Ignored when `start_line` is set."
                    },
                    "start_line": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "1-based line to start reading from. Combine with `line_count` to read a specific range without pulling the whole file. Text files only."
                    },
                    "line_count": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Number of lines to read starting from `start_line` (default: read to end). Use small values (e.g. 5–50) when you only need to inspect a known location."
                    }
                },
                "required": ["path"]
            }),
        },
        ToolDefinition {
            name: tn::WRITE_FILE.to_string(),
            description: "Create or update a file in the workspace. For NEW files or FULL rewrites only. NEVER use when edit_file can do the job — rewriting introduces subtle regressions.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g., artifacts/notes.md, apps/my-app/knowhow/api-reference.md, knowhow/domain.md). For knowhow/ paths, if the file already exists in shared (~/.lucidos/knowhow/) but not locally, the write updates the shared copy."
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message describing the intent (e.g., 'Add dark theme styles', 'Create job listing card layout')"
                    }
                },
                "required": ["path", "content"]
            }),
        },
        ToolDefinition {
            name: tn::EDIT_FILE.to_string(),
            description: "Make a targeted edit to an existing file. Two modes:\n\
                1. Text mode: old_string + new_string — search-and-replace for text files\n\
                2. JSON mode: json_path + new_value — surgical edit at a specific path for JSON files (.json, .slides, etc.)\n\
                JSON mode handles parsing, navigation, and re-serialization automatically. Use it for .slides and other JSON files to avoid escape and matching issues.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g., artifacts/notes.md, artifacts/deck.slides, apps/my-app/knowhow/api-reference.md, knowhow/domain.md). For knowhow/ paths, if the file already exists in shared (~/.lucidos/knowhow/) but not locally, the edit updates the shared copy."
                    },
                    "old_string": {
                        "type": "string",
                        "description": "Text mode: the exact text to find in the file"
                    },
                    "new_string": {
                        "type": "string",
                        "description": "Text mode: the text to replace it with (must be different from old_string)"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Text mode: replace all occurrences instead of just the first (default: false)"
                    },
                    "json_path": {
                        "type": "string",
                        "description": "JSON mode: path to the target value. Supports dot notation (`metadata.author.name`), array indices (`sections[1]`), quoted keys for non-identifier chars like dates or slugs (`dailyLog[\"2026-05-04\"]` or `dailyLog['2026-05-04']`), the JSONPath root marker (`$.streak`), and raw JSON Pointers (`/sections/1/title`). Mix freely — e.g. `habits[0].dailyLog[\"2026-05-04\"]`."
                    },
                    "new_value": {
                        "description": "JSON mode: the replacement value — can be any JSON type (string, number, object, array, boolean, null)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message describing the intent (e.g., 'Fix filter button spacing', 'Update slide 3 title')"
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
            description: "List all files in the workspace (artifacts and App UIs). Call ONCE at start if needed, then use the results.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
        },
        ToolDefinition {
            name: tn::GLOB_FILES.to_string(),
            description: "Find files in the workspace matching a glob pattern. Patterns are relative to data/ — use the same paths you'd see in list_files (e.g. 'apps/**/index.html', 'artifacts/*.md', '**/*.csv'). Searches artifacts/, apps/, knowhow/, triggers/. Prefer this over run_bash with find/ls.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern relative to data/. Examples: 'apps/**/index.html', 'artifacts/*.md', '**/*.csv'."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Max paths to return (default 200, max 1000). Returned paths are sorted; `truncated: true` in the result indicates the cap was hit."
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: tn::GREP_FILES.to_string(),
            description: "Search file contents using a regex (Rust regex crate syntax). Searches artifacts/, apps/, knowhow/, triggers/. Skips binary files. Prefer this over run_bash with rg/grep — it's structured and respects workspace ignore rules. Lines longer than 300 chars are truncated with `…` (PDF text dumps and unwrapped JSON commonly trip this), and the total returned text is capped at ~50 KB — narrow with `path_glob` if you hit `truncated: true` and need more detail.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regex pattern (Rust regex crate syntax). Case-sensitive by default."
                    },
                    "path_glob": {
                        "type": "string",
                        "description": "Optional glob to restrict which files are searched (e.g. 'apps/**/*.html', 'artifacts/*.md'). Defaults to all files under data/."
                    },
                    "case_insensitive": {
                        "type": "boolean",
                        "description": "Match case-insensitively (default false)."
                    },
                    "max_matches": {
                        "type": "integer",
                        "description": "Total match cap across all files (default 100, max 500). `truncated: true` in the result indicates the cap was hit."
                    },
                    "context_lines": {
                        "type": "integer",
                        "description": "Lines of context to return before/after each match (default 0, max 5)."
                    }
                },
                "required": ["pattern"]
            }),
        },
        ToolDefinition {
            name: tn::COPY_FILE.to_string(),
            description: "Copy a file within the workspace. Use this instead of read_file + write_file when you need to duplicate or move content — it handles the copy server-side without passing content through the conversation.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source": {
                        "type": "string",
                        "description": "Source path under data/ (e.g., artifacts/imported/report.txt)"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Destination path under data/ (e.g., artifacts/projects/report.txt)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message (e.g., 'Copy report to projects folder')"
                    }
                },
                "required": ["source", "destination"]
            }),
        },
        ToolDefinition {
            name: tn::DELETE_FILE.to_string(),
            description: "Delete a file from the workspace (recoverable from git history). Refuses plugin-owned paths; the error tells you the owning plugin id — call uninstall_plugin with that id instead so the user sees a confirm panel before deletion.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Relative path under data/ (e.g., artifacts/notes.md, apps/my-app/ui/main/old.js)"
                    },
                    "message": {
                        "type": "string",
                        "description": "Semantic commit message describing why (e.g., 'Remove unused utility module')"
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
            description: "Import a file from the local filesystem into the artifacts directory. The file will be copied, committed to git, and indexed for search. Refuses files larger than 100 MB — for bulk reference data, move the file to ~/.lucidos/data/<name>/ and pin the absolute path in the relevant app's knowhow (see system-knowhow/best-practices rule 8).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "source_path": {
                        "type": "string",
                        "description": "Absolute path to the source file on the local filesystem"
                    },
                    "destination": {
                        "type": "string",
                        "description": "Relative path in artifacts/imported/ (optional, defaults to original filename)"
                    }
                },
                "required": ["source_path"]
            }),
        },
    ]
}
