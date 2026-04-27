//! `lucidos send-thread` — POST a new thread to another (or this) Lucidos workspace.
//!
//! Defaults caller_* fields from env vars set by the engine when it spawns a
//! Claude Code subprocess: `$LUCIDOS_WORKSPACE` (basename → caller_workspace),
//! `$LUCIDOS_THREAD_ID` → caller_thread_id, `$LUCIDOS_EVENT_ID` → caller_event_id.
//! `mode` defaults to "agent" since this CLI is invoked from CC subprocesses;
//! override with --mode for engine-mode helpers.
//!
//! With `--parent`, the body emits `parent_thread_id` + `spawning_event_id`
//! instead, for same-workspace parent-with-callback spawns. Target workspace
//! basename must match $LUCIDOS_WORKSPACE basename in --parent mode (else error).

use std::path::PathBuf;

use crate::workspace::{read_api_port, BoxError};
use crate::SendThreadArgs;

pub(crate) fn run(args: SendThreadArgs) -> Result<(), BoxError> {
    let target_root = resolve_target(&args.to)?;
    let api_port = read_api_port(&target_root.join(".lucidos/ports"))?;

    let caller_workspace = std::env::var("LUCIDOS_WORKSPACE")
        .ok()
        .and_then(|p| PathBuf::from(p).file_name().map(|n| n.to_string_lossy().into_owned()));
    let caller_thread_id = std::env::var("LUCIDOS_THREAD_ID").ok();
    let caller_event_id = std::env::var("LUCIDOS_EVENT_ID").ok();

    if args.parent {
        // Same-workspace + parent-callback semantics. Verify target == caller.
        let target_basename = target_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let caller_basename = caller_workspace.as_deref().unwrap_or("");
        if target_basename != caller_basename {
            return Err(format!(
                "--parent requires --to to match $LUCIDOS_WORKSPACE basename ({}), got {}",
                caller_basename, target_basename
            ).into());
        }
    }

    let mut body = serde_json::json!({
        "message": args.message,
        "mode": args.mode.as_wire(),
    });
    let obj = body.as_object_mut().expect("json! created an object literal");
    if let Some(t) = args.title { obj.insert("title".into(), t.into()); }
    if args.cc { obj.insert("use_claude_code".into(), true.into()); }
    if let Some(m) = args.cc_model { obj.insert("cc_model".into(), m.into()); }
    if let Some(m) = args.model { obj.insert("model".into(), m.into()); }

    if args.parent {
        if let Some(t) = caller_thread_id { obj.insert("parent_thread_id".into(), t.into()); }
        if let Some(e) = caller_event_id { obj.insert("spawning_event_id".into(), e.into()); }
    } else {
        if let Some(w) = caller_workspace { obj.insert("caller_workspace".into(), w.into()); }
        if let Some(t) = caller_thread_id { obj.insert("caller_thread_id".into(), t.into()); }
        if let Some(e) = caller_event_id { obj.insert("caller_event_id".into(), e.into()); }
    }

    let scheme = if args.insecure_http { "http" } else { "https" };
    let url = format!("{}://localhost:{}/api/chat/stream", scheme, api_port);
    let client = crate::http::client()?;
    let resp = client.post(&url).json(&body).send()?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if !status.is_success() {
        return Err(format!("send-thread failed: HTTP {}: {}", status, text).into());
    }
    println!("{}", text);
    Ok(())
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
            name_or_path, candidate.display()
        ).into());
    }
    Ok(candidate)
}
