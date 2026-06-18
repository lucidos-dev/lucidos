//! `lucidos spawn-thread` — POST a new thread to another (or this) Lucidos workspace.
//!
//! Defaults caller_* fields from env vars set by the engine when it spawns a
//! coding-agent subprocess: `$LUCIDOS_WORKSPACE` (basename → caller_workspace),
//! `$LUCIDOS_THREAD_ID` → caller_thread_id, `$LUCIDOS_EVENT_ID` → caller_event_id.
//! `--repo` defaults from `$LUCIDOS_REPO` (the calling coding-agent thread's repo)
//! so a coding-agent subprocess automatically inherits its caller's repo without
//! callers having to pass it; callers can still override with `--repo <name>` or
//! pass `--repo ""` to force the target workspace's default repo.
//! `mode` defaults to "agent" since this CLI is invoked from coding-agent subprocesses;
//! override with --mode for engine-mode helpers.
//!
//! `--relation child` emits `parent_thread_id` + `spawning_event_id` instead
//! of `caller_*` fields, for same-workspace parent-with-callback spawns. The
//! target workspace basename must match `$LUCIDOS_WORKSPACE` basename in
//! `child` mode (else error). `--relation top` (the default) emits caller_*
//! and never gets a callback. `--parent` is a deprecated alias for
//! `--relation child` and prints a stderr warning. `sub` is also accepted
//! as a back-compat alias for `child`.
//!
//! The CLI generates the new thread's UUID up front and includes it in the
//! request body so it can print a `[title](thread:workspace/uuid)` markdown
//! link on stdout — the engine renders this as a clickable thread link when a
//! coding-agent subprocess includes it in its response.
//!
//! `--folder <path>` targets a folder instead of a repo: with `--cc`,
//! `--codex`, or `--coding-agent`, a `data/apps/<id>` value spawns an
//! *app coding-agent thread* — the engine
//! resolves the `folder` body field through the same `coding_agent_kind`
//! pipeline `run_coding_agent(folder=…)` uses (sparse-checkout worktree of the app
//! folder, Apply ff-merges to the workspace's main, no `/harden`, no engine
//! restart). `--folder` is mutually exclusive with `--repo` (enforced by clap)
//! and suppresses the `$LUCIDOS_REPO` default, because the engine rejects a
//! request carrying both `repo_id` and `folder`.

use std::path::PathBuf;

use crate::workspace::{read_ports, BoxError};
use crate::{CliRelation, SpawnThreadArgs};

pub(crate) fn run(args: SpawnThreadArgs) -> Result<(), BoxError> {
    let selected_coding_agent = if args.codex {
        Some(crate::CliCodingAgent::Codex)
    } else {
        args.coding_agent
    };
    let use_coding_agent = args.cc || selected_coding_agent.is_some();

    // `--folder` only makes sense for coding-agent threads (it targets an app
    // folder for a coding-agent worktree). Reject early with a clear message — clap
    // already rejects `--folder` together with `--repo`.
    if args.folder.is_some() && !use_coding_agent {
        return Err(
            "--folder requires --cc, --codex, or --coding-agent (folder targeting only applies to coding-agent threads)".into(),
        );
    }

    let target_root = resolve_target(&args.to)?;
    let (api_port, ports_proto) = read_ports(&target_root.join(".lucidos/ports"))?;

    let caller_workspace = std::env::var("LUCIDOS_WORKSPACE").ok().and_then(|p| {
        PathBuf::from(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    });
    let caller_thread_id = std::env::var("LUCIDOS_THREAD_ID").ok();
    let caller_event_id = std::env::var("LUCIDOS_EVENT_ID").ok();

    let target_basename = target_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Resolve relation: explicit `--relation` wins; `--parent` is a
    // deprecated alias for `--relation child` (warn once on stderr); otherwise
    // default to `top` so existing cross-workspace recipes keep their
    // fire-and-forget behavior.
    let relation = match (args.relation, args.parent) {
        (Some(r), _) => r,
        (None, true) => {
            eprintln!("warning: --parent is deprecated; use --relation child");
            CliRelation::Child
        }
        (None, false) => CliRelation::Top,
    };

    if matches!(relation, CliRelation::Child) {
        let caller_basename = caller_workspace.as_deref().unwrap_or("");
        if target_basename != caller_basename {
            return Err(format!(
                "--relation child requires --to to match $LUCIDOS_WORKSPACE basename ({}), got {} \
                 — same-workspace only (callbacks across workspaces are unsupported)",
                caller_basename, target_basename
            )
            .into());
        }
    }

    // Resolve --repo: explicit `Some` (including empty string) wins; otherwise
    // fall back to $LUCIDOS_REPO (the engine sets this on every coding-agent
    // subprocess to the calling thread's repo name). Passing `--repo ""` explicitly
    // requests the workspace default even when the env var is set.
    //
    // `--folder` suppresses the env-var default entirely: the engine 400s on a
    // request carrying both `repo_id` and `folder`, and since it sets
    // $LUCIDOS_REPO on every subprocess, a folder spawn would otherwise always
    // collide. (`--folder` + explicit `--repo` is already rejected by clap.)
    let repo = if args.folder.is_some() {
        None
    } else {
        match args.repo {
            Some(s) => Some(s),
            None => std::env::var("LUCIDOS_REPO").ok(),
        }
        .filter(|s| !s.is_empty())
    };

    // Generate the new thread's UUID up front so we can print the link without
    // a second request. The engine accepts a client-supplied thread_id and
    // creates the thread under that id (used for both id-by-link and idempotency).
    let thread_id = uuid::Uuid::new_v4().to_string();

    let mut body = serde_json::json!({
        "message": args.message,
        "mode": args.mode.as_wire(),
        "thread_id": thread_id,
    });
    let obj = body
        .as_object_mut()
        .expect("json! created an object literal");
    if let Some(ref t) = args.title {
        obj.insert("title".into(), t.clone().into());
    }
    if use_coding_agent {
        obj.insert("use_coding_agent".into(), true.into());
    }
    if let Some(agent) = selected_coding_agent {
        obj.insert("coding_agent".into(), agent.as_wire().into());
    }
    if let Some(m) = args.cc_model {
        obj.insert("cc_model".into(), m.into());
    }
    if let Some(m) = args.model {
        obj.insert("model".into(), m.into());
    }
    if let Some(r) = repo {
        obj.insert("repo_id".into(), r.into());
    }
    // App coding-agent thread targeting: the engine resolves `folder` through
    // the shared `coding_agent_kind` pipeline (same as
    // `run_coding_agent(folder=…)`).
    if let Some(ref f) = args.folder {
        obj.insert("folder".into(), f.clone().into());
    }

    match relation {
        CliRelation::Child => {
            if let Some(t) = caller_thread_id {
                obj.insert("parent_thread_id".into(), t.into());
            }
            if let Some(e) = caller_event_id {
                obj.insert("spawning_event_id".into(), e.into());
            }
        }
        CliRelation::Top => {
            if let Some(w) = caller_workspace {
                obj.insert("caller_workspace".into(), w.into());
            }
            if let Some(t) = caller_thread_id {
                obj.insert("caller_thread_id".into(), t.into());
            }
            if let Some(e) = caller_event_id {
                obj.insert("caller_event_id".into(), e.into());
            }
        }
    }

    let scheme = if args.insecure_http {
        "http"
    } else {
        &ports_proto
    };
    let url = format!("{}://localhost:{}/api/v1/chat/stream", scheme, api_port);
    let client = crate::http::client()?;
    let start = std::time::Instant::now();
    let resp = client
        .post(&url)
        .json(&body)
        .send()
        .map_err(|e| crate::http::format_request_error("POST", &url, &e, start.elapsed()))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("spawn-thread failed: HTTP {}: {}", status, text).into());
    }

    // Print the spawned thread as a markdown link the receiving frontend
    // renders as a clickable thread navigation (see renderMarkdown.ts —
    // `thread:workspace/uuid`). The link is what the LLM should include in
    // its response so the user can click through to the spawned thread.
    let label = link_label(args.title.as_deref(), &args.message);
    println!("[{}](thread:{}/{})", label, target_basename, thread_id);
    Ok(())
}

/// Pick the markdown link label: the explicit title if given, otherwise the
/// first line of the message clipped to 60 characters. `]` and newlines are
/// stripped because either would close the markdown link prematurely (the
/// title path can carry a multi-line string that the message-fallback path
/// can't, since the latter already does `.lines().next()`).
fn link_label(title: Option<&str>, message: &str) -> String {
    let raw = match title {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => {
            let first_line = message.lines().next().unwrap_or("").trim();
            let clip_at = first_line
                .char_indices()
                .nth(60)
                .map(|(i, _)| i)
                .unwrap_or(first_line.len());
            if clip_at < first_line.len() {
                format!("{}…", &first_line[..clip_at])
            } else {
                first_line.to_string()
            }
        }
    };
    raw.chars()
        .filter(|c| *c != ']' && *c != '\n' && *c != '\r')
        .collect()
}

fn resolve_target(name_or_path: &str) -> Result<PathBuf, BoxError> {
    let p = PathBuf::from(name_or_path);
    if p.is_absolute() {
        return Ok(p);
    }
    let root = match std::env::var("LUCIDOS_WORKSPACES_ROOT") {
        Ok(v) => PathBuf::from(v),
        Err(_) => dirs::home_dir()
            .ok_or_else(|| -> BoxError {
                "Cannot resolve target workspace: no $LUCIDOS_WORKSPACES_ROOT and no home directory. \
                 Pass an absolute path to --to or set $LUCIDOS_WORKSPACES_ROOT.".into()
            })?
            .join("workspaces"),
    };
    let candidate = root.join(name_or_path);
    if !candidate.join(".lucidos/ports").is_file() {
        return Err(format!(
            "Target workspace '{}' not found at {} (no .lucidos/ports).",
            name_or_path,
            candidate.display()
        )
        .into());
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::link_label;

    #[test]
    fn label_uses_title_when_given() {
        assert_eq!(
            link_label(Some("Fix repo flag"), "irrelevant body"),
            "Fix repo flag"
        );
    }

    #[test]
    fn label_falls_back_to_first_message_line() {
        assert_eq!(link_label(None, "do the thing\nwith arg"), "do the thing");
    }

    #[test]
    fn label_clips_long_first_line() {
        let label = link_label(None, &"x".repeat(120));
        assert_eq!(label.chars().count(), 61);
        assert!(label.ends_with('…'));
    }

    #[test]
    fn label_strips_closing_bracket_to_keep_markdown_balanced() {
        assert_eq!(link_label(Some("foo] bar"), "msg"), "foo bar");
    }

    #[test]
    fn label_strips_newlines_to_keep_link_on_one_line() {
        assert_eq!(
            link_label(Some("line1\nline2\rline3"), "msg"),
            "line1line2line3"
        );
    }

    #[test]
    fn label_falls_back_to_message_when_title_is_blank() {
        assert_eq!(link_label(Some("   "), "real message"), "real message");
    }
}
