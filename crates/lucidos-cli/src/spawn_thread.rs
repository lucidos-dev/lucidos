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
//! and never gets a callback. `sub` is also accepted as a back-compat alias
//! for `child`.
//!
//! The CLI generates the new thread's UUID up front and includes it in the
//! request body so it can print a `[title](thread:workspace/uuid)` markdown
//! link on stdout — the engine renders this as a clickable thread link when a
//! coding-agent subprocess includes it in its response.
//!
//! `--reasoning-effort <level>` pins the coding agent's thinking level for this
//! spawn only, overriding whatever default the backend would otherwise read.
//! It needs a coding-agent flag, and takes one of [`EFFORT_LEVELS`].
//!
//! `--folder <path>` targets a folder instead of a repo: with `--coding-agent`,
//! `--codex`, or `--cc`, a `data/apps/<id>` value spawns an
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

/// Reasoning levels `--reasoning-effort` accepts, ascending.
///
/// Both coding-agent backends offer exactly these five, so the flag is
/// backend-neutral. The chat ladder's `none` is deliberately absent: neither
/// backend menu has it.
///
/// This crate cannot call the engine's `is_valid_effort`, so
/// `effort_levels_match_the_backend_menus` pins the list against the two menu
/// files that validator reads.
pub(crate) const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// The two picker menus the engine validates a coding-agent effort against.
/// Baked in at compile time, so the CLI needs no path lookup at runtime.
const CC_MENU: &str = include_str!("../../lucidos-engine/src/runtime/cc_menu_options.json");
const CODEX_MENU: &str = include_str!("../../lucidos-engine/src/runtime/codex_menu_options.json");

/// The `reasoning_efforts` rows of one menu file.
///
/// Empty on a malformed file, which reads downstream as "no restriction". That
/// direction is deliberate: the engine still drops a level its model rejects,
/// so a broken parse costs a refusal the CLI could have made, never a wrong
/// refusal. `effort_levels_match_the_backend_menus` fails if either file stops
/// parsing, so the empty case cannot reach a release.
fn effort_rows(menu: &str) -> Vec<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(menu)
        .ok()
        .and_then(|parsed| parsed["reasoning_efforts"].as_array().cloned())
        .unwrap_or_default()
}

/// The models `effort` is restricted to, or `None` when every model offers it.
///
/// Read by the same rule the engine's `validate_codex_effort` applies to these
/// same rows. That function DROPS a level the model does not offer, rather than
/// refusing, because an unsupported value kills a Codex turn outright. The
/// session still records the level that was asked for, so the picker shows a
/// tier the backend never ran. Refusing here is what keeps the two honest.
fn models_offering(menu: &str, effort: &str) -> Option<Vec<String>> {
    let row = effort_rows(menu)
        .into_iter()
        .find(|row| row["value"] == effort)?;
    let allowed = row["supported_models"].as_array()?;
    Some(
        allowed
            .iter()
            .filter_map(|m| m.as_str().map(str::to_string))
            .collect(),
    )
}

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
            "--folder requires --coding-agent, --codex, or --cc (folder targeting only applies to coding-agent threads)".into(),
        );
    }

    // A chat thread's effort comes off a different ladder, which has a `none`
    // tier these five levels do not. So the flag would mean something else.
    if args.reasoning_effort.is_some() && !use_coding_agent {
        return Err(
            "--reasoning-effort requires --coding-agent, --codex, or --cc (a chat thread's reasoning level is not set here)".into(),
        );
    }

    if let Some(effort) = args.reasoning_effort.as_deref() {
        check_effort_fits_model(
            selected_coding_agent,
            args.coding_agent_model.as_deref(),
            effort,
        )?;
    }

    let target_root = resolve_target(&args.to)?;
    let target_ports = target_root.join(".lucidos/ports");
    let (api_port, recorded_proto) = read_ports(&target_ports)?;
    let proto_assumed = recorded_proto.is_none();
    let ports_proto = recorded_proto.unwrap_or_else(|| crate::workspace::DEFAULT_PROTO.to_string());

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

    // Resolve relation: explicit `--relation` wins; otherwise default to `top`
    // so existing cross-workspace recipes keep their fire-and-forget behavior.
    let relation = args.relation.unwrap_or(CliRelation::Top);

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
    if let Some(m) = args.coding_agent_model {
        obj.insert("cc_model".into(), m.into());
    }
    if let Some(m) = args.model {
        obj.insert("model".into(), m.into());
    }
    // Pins this spawn's reasoning level, ahead of the backend's own default.
    // The engine resolves the payload value first (`run_direct_agent`), then
    // hands it to the subprocess as `CLAUDE_CODE_EFFORT_LEVEL`.
    if let Some(e) = args.reasoning_effort {
        obj.insert("reasoning_effort".into(), e.into());
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
        // This is the ONE subcommand that deliberately talks to a workspace
        // other than its own, so it asserts the TARGET rather than letting the
        // `$LUCIDOS_WORKSPACE` default through (which would name the caller and
        // earn a 409 from the receiving engine). A per-request header wins over
        // a default of the same name.
        .header(crate::http::HEADER_TARGET_WORKSPACE, &target_basename)
        .json(&body)
        .send()
        // A scheme this had to guess is the likeliest reason for a transport
        // failure here, so the message names the file that failed to say.
        .map_err(|e| {
            let mut msg = crate::http::format_request_error("POST", &url, &e, start.elapsed());
            if proto_assumed && !args.insecure_http {
                msg.push_str(&crate::workspace::assumed_proto_note(&target_ports));
            }
            msg
        })?;
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

/// Refuse a level the spawn cannot actually run at.
///
/// Only the backend's own menu decides. Claude Code restricts no level, so this
/// only ever fires for Codex, whose `max` names three models.
///
/// An UNNAMED model is refused too, which is the non-obvious half. The engine
/// tests the restriction with `model.is_some_and(...)`. So a restricted level
/// with no model is dropped before Codex is consulted, whatever that config
/// would have picked. Sending it gains nothing.
fn check_effort_fits_model(
    agent: Option<crate::CliCodingAgent>,
    model: Option<&str>,
    effort: &str,
) -> Result<(), BoxError> {
    let menu = match agent {
        Some(crate::CliCodingAgent::Codex) => CODEX_MENU,
        _ => CC_MENU,
    };
    let Some(allowed) = models_offering(menu, effort) else {
        return Ok(());
    };
    // Empty and the `default` sentinel both mean "let the backend config pick",
    // and neither can match a name in the list.
    let named = model.filter(|m| !m.is_empty() && *m != "default");
    if named.is_some_and(|m| allowed.iter().any(|a| a == m)) {
        return Ok(());
    }
    Err(format!(
        "--reasoning-effort {} runs only on: {}. Pass --coding-agent-model with one of them; \
         on any other model the backend drops the level and runs at its own default.",
        effort,
        allowed.join(", ")
    )
    .into())
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

/// The directory a bare `--to <name>` is resolved against, in priority order:
/// an explicit `LUCIDOS_WORKSPACES_ROOT`, then the directory holding
/// `LUCIDOS_WORKSPACE`, then `~/workspaces`.
///
/// Mirrors `workspaces_root_from_env` in the engine's
/// `engine::http::workspace_resolver`, which owns the full reasoning. Both
/// must agree, or `--to <ws>` and `run_coding_agent(workspace=<ws>)` resolve to
/// different places. The short version: a packaged install keeps its
/// workspaces under `<app-data>/workspaces`, and the middle step is what finds
/// a sibling there without a new env var (ADR 0136).
fn workspaces_root(
    explicit: Option<std::ffi::OsString>,
    workspace: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(root) = explicit {
        return Some(PathBuf::from(root));
    }
    if let Some(parent) = workspace
        .map(PathBuf::from)
        .as_deref()
        .and_then(std::path::Path::parent)
    {
        return Some(parent.to_path_buf());
    }
    home.map(|h| h.join("workspaces"))
}

fn resolve_target(name_or_path: &str) -> Result<PathBuf, BoxError> {
    let p = PathBuf::from(name_or_path);
    if p.is_absolute() {
        return Ok(p);
    }
    let root = workspaces_root(
        std::env::var_os("LUCIDOS_WORKSPACES_ROOT"),
        std::env::var_os("LUCIDOS_WORKSPACE"),
        dirs::home_dir(),
    )
    .ok_or_else(|| -> BoxError {
        "Cannot resolve target workspace: no $LUCIDOS_WORKSPACES_ROOT, no $LUCIDOS_WORKSPACE \
         and no home directory. Pass an absolute path to --to."
            .into()
    })?;
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
    use super::{
        check_effort_fits_model, effort_rows, link_label, models_offering, workspaces_root,
        CC_MENU, CODEX_MENU, EFFORT_LEVELS,
    };
    use crate::CliCodingAgent;
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::path::PathBuf;

    fn menu_levels(menu: &str) -> BTreeSet<String> {
        let rows = effort_rows(menu);
        assert!(!rows.is_empty(), "menu file must parse and declare rows");
        rows.iter()
            .map(|e| {
                e["value"]
                    .as_str()
                    .expect("every effort row has a value")
                    .to_string()
            })
            .collect()
    }

    /// The flag must accept exactly what both backends offer.
    ///
    /// The engine's `is_valid_effort` reads these same files, and this crate
    /// cannot call it. Drift either way is a real bug. A level the flag refuses
    /// is unreachable from the CLI. One it accepts but no menu has reaches the
    /// subprocess as an env var nothing honours.
    ///
    /// If the two backends ever diverge, this fails rather than silently taking
    /// a union. Make the flag backend-aware at that point.
    #[test]
    fn effort_levels_match_the_backend_menus() {
        let ours: BTreeSet<String> = EFFORT_LEVELS.iter().map(|s| s.to_string()).collect();
        assert_eq!(
            menu_levels(CC_MENU),
            ours,
            "cc_menu_options.json moved: update EFFORT_LEVELS"
        );
        assert_eq!(
            menu_levels(CODEX_MENU),
            ours,
            "codex_menu_options.json moved: update EFFORT_LEVELS"
        );
    }

    /// The restriction the refusal is built on. Codex names three models for
    /// `max`; every other level, and every Claude Code level, is universal.
    ///
    /// `system-knowhow/lucidos-cli.md` tells the reader exactly this, so a menu
    /// that restricts a second level has to fail here rather than silently make
    /// that page wrong.
    #[test]
    fn only_codex_max_names_the_models_that_offer_it() {
        let codex_max = models_offering(CODEX_MENU, "max").expect("codex max names its models");
        assert!(
            codex_max.iter().all(|m| m.starts_with("gpt-5.6")),
            "unexpected models for codex max: {:?}",
            codex_max
        );
        for level in EFFORT_LEVELS.iter().filter(|l| **l != "max") {
            assert_eq!(
                models_offering(CODEX_MENU, level),
                None,
                "only Codex `max` is restricted, so {} must stay universal",
                level
            );
        }
        for level in EFFORT_LEVELS {
            assert_eq!(
                models_offering(CC_MENU, level),
                None,
                "Claude Code restricts no level, so {} must stay universal",
                level
            );
        }
    }

    /// The engine drops a level the model rejects, and the session still
    /// records the level asked for. So a pin the CLI knows cannot hold is
    /// refused here, where the message can name the models that do offer it.
    #[test]
    fn an_effort_the_named_codex_model_lacks_is_refused() {
        let err = check_effort_fits_model(Some(CliCodingAgent::Codex), Some("gpt-5.5"), "max")
            .expect_err("gpt-5.5 does not offer max");
        let msg = err.to_string();
        assert!(
            msg.contains("gpt-5.6-sol"),
            "names a model that fits: {msg}"
        );
    }

    /// A model that does offer the level, and every Claude Code pairing, pass.
    #[test]
    fn a_supported_pairing_is_accepted() {
        assert!(
            check_effort_fits_model(Some(CliCodingAgent::Codex), Some("gpt-5.6-sol"), "max")
                .is_ok()
        );
        assert!(
            check_effort_fits_model(Some(CliCodingAgent::ClaudeCode), Some("opus"), "max").is_ok()
        );
        assert!(check_effort_fits_model(None, Some("opus"), "max").is_ok());
        assert!(check_effort_fits_model(Some(CliCodingAgent::Codex), None, "high").is_ok());
    }

    /// The engine tests a restricted level with `model.is_some_and(...)`, so an
    /// unnamed model drops it before Codex is consulted. Whatever that config
    /// would have picked never gets a say, so the CLI refuses rather than
    /// letting a pin through that provably cannot hold.
    #[test]
    fn a_restricted_level_needs_a_named_model() {
        for model in [None, Some(""), Some("default")] {
            let err = check_effort_fits_model(Some(CliCodingAgent::Codex), model, "max")
                .expect_err("codex max always drops without a named model");
            assert!(
                err.to_string().contains("--coding-agent-model"),
                "must say what to pass: {model:?}"
            );
        }
    }

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    /// A packaged workspace sits under app-support, so resolving beside the
    /// caller's own workspace is what finds a sibling by bare name. Must match
    /// the engine's `workspaces_root_from_env`, or the CLI and the
    /// `run_coding_agent(workspace=…)` tool disagree about where `<name>` is.
    #[test]
    fn a_bare_name_resolves_beside_the_callers_own_workspace() {
        assert_eq!(
            workspaces_root(
                None,
                Some(os("/app-data/workspaces/other")),
                Some(PathBuf::from("/h"))
            ),
            Some(PathBuf::from("/app-data/workspaces"))
        );
    }

    /// An operator who set the root explicitly means it.
    #[test]
    fn an_explicit_root_beats_the_callers_workspace() {
        assert_eq!(
            workspaces_root(
                Some(os("/elsewhere")),
                Some(os("/app-data/workspaces/other")),
                Some(PathBuf::from("/h"))
            ),
            Some(PathBuf::from("/elsewhere"))
        );
    }

    /// A dev checkout that sets neither variable keeps the legacy default.
    #[test]
    fn home_workspaces_is_still_the_last_resort() {
        assert_eq!(
            workspaces_root(None, None, Some(PathBuf::from("/h"))),
            Some(PathBuf::from("/h/workspaces"))
        );
    }

    /// Nothing to resolve against is an error the caller reports, not a bare
    /// relative path joined onto nothing.
    #[test]
    fn no_source_at_all_yields_no_root() {
        assert_eq!(workspaces_root(None, None, None), None);
    }

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
