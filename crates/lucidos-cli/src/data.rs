use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::http::{client as http_client, send_expect_success};
use crate::workspace::{BoxError, Workspace};

/// Sub-directories of the workspace's `data/` that the CLI accepts as
/// already-prefixed when resolving paths. The frontend has a similar list at
/// `crates/lucidos-app/src/store/actions/artifacts.ts`, but it includes
/// `system-knowhow/` (engine-shipped, served from the engine repo); that prefix
/// does *not* belong here because it isn't a workspace data sub-directory.
const DATA_PREFIXES: &[&str] = &["artifacts/", "knowhow/", "apps/", "triggers/"];

/// Validate `relative` and return its normalized, `data/`-rooted store path
/// (e.g. `loose.txt` → `artifacts/loose.txt`). This store-relative form — with
/// NO scheme — is the canonical clickable link target in Lucidos chat; see
/// `chat_link` / `cmd_write`.
pub(crate) fn normalize_data_path(relative: &str) -> Result<String, BoxError> {
    let trimmed = relative.trim();
    if trimmed.is_empty() {
        return Err("path is empty".into());
    }
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(format!("path must be relative, got {:?}", relative).into());
    }
    if trimmed.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(format!("path may not contain '..' segments, got {:?}", relative).into());
    }
    Ok(normalize(trimmed))
}

pub(crate) fn resolve_data_path(ws: &Workspace, relative: &str) -> Result<PathBuf, BoxError> {
    Ok(ws.data_dir().join(normalize_data_path(relative)?))
}

/// Build the ready-to-paste clickable Lucidos chat link for a freshly written
/// data file. The target is the bare, `data/`-rooted store path (NO scheme):
/// the frontend's path linkifier rewrites it into a file-preview link. An
/// invented `artifact:` / `file:` scheme dead-ends — no handler claims it — so
/// the link MUST stay scheme-less. Label defaults to the file's basename.
fn chat_link(normalized: &str) -> String {
    let label = normalized.rsplit('/').next().unwrap_or(normalized);
    format!("[{}]({})", label, normalized)
}

fn normalize(path: &str) -> String {
    if DATA_PREFIXES.iter().any(|p| path.starts_with(p)) {
        path.to_string()
    } else {
        format!("artifacts/{}", path)
    }
}

pub(crate) fn cmd_path(ws: &Workspace, relative: &str, mkdir: bool) -> Result<(), BoxError> {
    let abs = resolve_data_path(ws, relative)?;
    if mkdir {
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dir {}: {}", parent.display(), e))?;
        }
    }
    println!("{}", abs.display());
    Ok(())
}

pub(crate) enum WriteSource {
    Stdin,
    File(PathBuf),
}

/// Percent-encode a `data/`-rooted store path for use as URL path segments.
/// Keeps `/` as the separator and the RFC 3986 unreserved set verbatim; encodes
/// everything else. Without this a perfectly ordinary artifact name breaks the
/// request: a space makes an invalid URL, and a `#` would be parsed as the
/// start of a fragment and silently truncate the path. Axum percent-decodes
/// `Path<String>` on the way in, so the engine sees the original bytes.
fn encode_path_segments(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Write content to a `data/`-rooted path in the PARENT workspace.
///
/// Goes through the engine's `PUT /api/v1/data/*path` rather than writing the
/// file directly, because that route is the *announced* write path: it commits
/// the file to the `data/` repo and emits `DataFileWritten` plus, for an
/// `artifacts/` path, the paired `Artifact*` entity event (ADR 0032, "a state
/// write owns its announcement"). A direct `std::fs::write` here skipped all
/// three, so a file this command created was invisible to the Files panel, to
/// the memory index, to an `on_event: ArtifactCreated` trigger, and to git,
/// until something else forced a reload. Worse, the chat link this very
/// function prints then failed to resolve against the frontend's artifact cache
/// and reloaded the whole workspace on click.
///
/// ADR 0032's registry (`core/announced_surfaces.rs`) covers `data/` writers in
/// the ENGINE crate, so it could never have caught this one: the CLI is a
/// separate binary. Routing through the engine makes it a caller of the
/// registered writer instead of a second, unregistered one.
///
/// Consequence: this needs a running engine, like every other mutating
/// subcommand (`events emit`, `notify`, `changes apply`). A failed write is a
/// hard error and prints no chat link, rather than a silent local write nothing
/// in the workspace knows about.
pub(crate) fn cmd_write(
    ws: &Workspace,
    relative: &str,
    source: WriteSource,
) -> Result<(), BoxError> {
    let normalized = normalize_data_path(relative)?;
    let abs = ws.data_dir().join(&normalized);

    let bytes = match source {
        WriteSource::Stdin => {
            let mut buf = Vec::new();
            io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            buf
        }
        WriteSource::File(path) => std::fs::read(&path)
            .map_err(|e| format!("Failed to read source file {}: {}", path.display(), e))?,
    };

    let url = format!(
        "{}/api/v1/data/{}",
        ws.base_url(),
        encode_path_segments(&normalized)
    );
    let req = http_client()?
        .put(&url)
        .header("Content-Type", "application/octet-stream")
        .body(bytes);
    send_expect_success("PUT", &url, req)?;

    // Echo the resolved absolute path on stderr so callers see exactly what was
    // written, keeping stdout clean for the clickable link below.
    writeln!(io::stderr(), "{}", abs.display())
        .map_err(|e| format!("Failed to write status to stderr: {}", e))?;

    // Print a ready-to-paste clickable Lucidos chat link on stdout, mirroring
    // `lucidos spawn-thread`. This gives the agent a canonical, working link to
    // hand the user instead of inventing an `artifact:`/`file:` scheme that the
    // frontend has no handler for and that dead-ends on click.
    println!("{}", chat_link(&normalized));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_at(root: PathBuf) -> Workspace {
        Workspace {
            root,
            api_port: 0,
            proto: "https".to_string(),
            api_base_override: None,
        }
    }

    #[test]
    fn keeps_known_prefix() {
        let ws = ws_at(PathBuf::from("/ws"));
        let abs = resolve_data_path(&ws, "artifacts/ua/report.html").unwrap();
        assert_eq!(abs, PathBuf::from("/ws/data/artifacts/ua/report.html"));
    }

    #[test]
    fn keeps_each_known_prefix() {
        let ws = ws_at(PathBuf::from("/ws"));
        for prefix in DATA_PREFIXES {
            let path = format!("{}{}", prefix, "child/file.txt");
            let abs = resolve_data_path(&ws, &path).unwrap();
            assert_eq!(abs, PathBuf::from(format!("/ws/data/{}", path)));
        }
    }

    #[test]
    fn prepends_artifacts_when_missing_prefix() {
        let ws = ws_at(PathBuf::from("/ws"));
        let abs = resolve_data_path(&ws, "report.html").unwrap();
        assert_eq!(abs, PathBuf::from("/ws/data/artifacts/report.html"));
    }

    #[test]
    fn rejects_absolute_path() {
        let ws = ws_at(PathBuf::from("/ws"));
        assert!(resolve_data_path(&ws, "/etc/passwd").is_err());
    }

    #[test]
    fn rejects_dot_dot_segment() {
        let ws = ws_at(PathBuf::from("/ws"));
        assert!(resolve_data_path(&ws, "artifacts/../../etc/passwd").is_err());
    }

    #[test]
    fn rejects_empty_path() {
        let ws = ws_at(PathBuf::from("/ws"));
        assert!(resolve_data_path(&ws, "   ").is_err());
    }

    // Landing the bytes on disk (and creating parent dirs) is the engine's job
    // now, not this file's: see `cmd_write`. Those assertions live in
    // `tests/data_write_lands_in_parent.rs`, against a stub engine.

    #[test]
    fn encode_path_segments_keeps_separators_and_unreserved_chars() {
        assert_eq!(
            encode_path_segments("artifacts/pr-review/pr_1582/index.html"),
            "artifacts/pr-review/pr_1582/index.html"
        );
    }

    #[test]
    fn encode_path_segments_escapes_a_space() {
        assert_eq!(
            encode_path_segments("artifacts/quarterly report.md"),
            "artifacts/quarterly%20report.md"
        );
    }

    #[test]
    fn encode_path_segments_escapes_the_fragment_and_query_markers() {
        // Left raw, a `#` truncates the URL at the fragment and a `?` starts a
        // query string, so the engine would receive a different path than the
        // one written.
        assert_eq!(
            encode_path_segments("artifacts/a#b?c.md"),
            "artifacts/a%23b%3Fc.md"
        );
    }

    #[test]
    fn encode_path_segments_escapes_non_ascii_bytewise() {
        // Percent-encoding is defined over BYTES, so a multi-byte char becomes
        // one escape per UTF-8 byte.
        assert_eq!(
            encode_path_segments("artifacts/å.md"),
            "artifacts/%C3%A5.md"
        );
    }

    #[test]
    fn normalize_data_path_prepends_artifacts_when_missing_prefix() {
        assert_eq!(
            normalize_data_path("report.html").unwrap(),
            "artifacts/report.html"
        );
    }

    #[test]
    fn normalize_data_path_keeps_known_prefix() {
        assert_eq!(normalize_data_path("knowhow/x.md").unwrap(), "knowhow/x.md");
    }

    #[test]
    fn chat_link_uses_basename_label_and_bare_path_target() {
        // No scheme on the target — a bare store path is what the frontend
        // linkifier rewrites to a file preview; `artifact:`/`file:` dead-ends.
        assert_eq!(
            chat_link("artifacts/ticket-workflow/node-types-and-attributes.md"),
            "[node-types-and-attributes.md](artifacts/ticket-workflow/node-types-and-attributes.md)"
        );
    }

    #[test]
    fn chat_link_handles_top_level_file() {
        assert_eq!(
            chat_link("artifacts/report.html"),
            "[report.html](artifacts/report.html)"
        );
    }
}
