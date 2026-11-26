use std::path::{Path, PathBuf};

use crate::http::ConnectRetry;
use crate::workspace::{read_ports, BoxError};
use crate::{CliMode, CliRelation, SpawnThreadArgs};

pub(crate) const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

pub(crate) const CC_MENU: &str = include_str!("../../lucidos-core/src/llm/cc_menu_options.json");
pub(crate) const CODEX_MENU: &str =
    include_str!("../../lucidos-core/src/llm/codex_menu_options.json");

fn effort_rows(menu: &str) -> Vec<serde_json::Value> {
    serde_json::from_str::<serde_json::Value>(menu)
        .ok()
        .and_then(|parsed| parsed["reasoning_efforts"].as_array().cloned())
        .unwrap_or_default()
}

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

enum SpawnDestination {
    Direct(PathBuf),
    Routed(crate::routing::Selection),
}

pub(crate) fn run(args: SpawnThreadArgs) -> Result<(), BoxError> {
    check_actor_mode(
        args.mode,
        std::env::var_os("LUCIDOS_AGENT_ORIGIN_TOKEN").is_some(),
    )?;

    let selected_coding_agent = if args.codex {
        Some(crate::CliCodingAgent::Codex)
    } else {
        args.coding_agent
    };
    let use_coding_agent = args.cc || selected_coding_agent.is_some();

    if args.folder.is_some() && !use_coding_agent {
        return Err(
            "--folder requires --coding-agent, --codex, or --cc (folder targeting only applies to coding-agent threads)".into(),
        );
    }

    if args.reasoning_effort.is_some() && !use_coding_agent {
        return Err(
            "--reasoning-effort requires --coding-agent, --codex, or --cc (a chat thread's reasoning level is not set here)".into(),
        );
    }

    if args.integrated {
        check_integrated_spawn(
            args.relation,
            use_coding_agent,
            args.folder.is_some(),
            args.mode,
        )?;
    }

    if let Some(effort) = args.reasoning_effort.as_deref() {
        check_effort_fits_model(
            selected_coding_agent,
            args.coding_agent_model.as_deref(),
            effort,
        )?;
    }

    require_input(&args.message, &args.images)?;
    let images = inline_images(&args.images)?;

    let wants_route =
        args.via.is_some() || (!Path::new(&args.to).is_absolute() && args.to.contains('@'));
    let (destination, target_basename) = if wants_route {
        let workspace = crate::routing::routed_workspace_name(&args.to)
            .filter(|_| !args.insecure_http)
            .ok_or("Routed spawns need --to <workspace[@remote]> and use registered gateway transport settings, without --insecure-http.")?;
        let route = crate::routing::Selection {
            via: args.via,
            to: Some(args.to.clone()),
        };
        (SpawnDestination::Routed(route), workspace.to_string())
    } else {
        let root = resolve_target(&args.to)?;
        let workspace = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        (SpawnDestination::Direct(root), workspace)
    };

    let caller_workspace = std::env::var("LUCIDOS_WORKSPACE").ok().and_then(|p| {
        PathBuf::from(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
    });
    let caller_thread_id = std::env::var("LUCIDOS_THREAD_ID").ok();
    let caller_event_id = std::env::var("LUCIDOS_EVENT_ID").ok();

    let relation = args.relation.unwrap_or(CliRelation::Top);
    if matches!(&destination, SpawnDestination::Routed(_)) && matches!(relation, CliRelation::Child)
    {
        return Err(
            "Routed spawns start top-level threads. Child callbacks remain within one workspace."
                .into(),
        );
    }

    if args.wake_mode.is_some() && !matches!(relation, CliRelation::Child) {
        return Err("--wake-mode needs --relation child: a top thread never reports back".into());
    }

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

    let repo = if args.folder.is_some() {
        None
    } else {
        match args.repo {
            Some(s) => Some(s),
            None => std::env::var("LUCIDOS_REPO").ok(),
        }
        .filter(|s| !s.is_empty())
    };

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
    if !images.is_empty() {
        obj.insert("images".into(), images.into());
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
    if let Some(e) = args.reasoning_effort {
        obj.insert("reasoning_effort".into(), e.into());
    }
    if let Some(r) = repo {
        obj.insert("repo_id".into(), r.into());
    }
    if let Some(ref f) = args.folder {
        obj.insert("folder".into(), f.clone().into());
    }
    if args.integrated {
        obj.insert("integrated".into(), true.into());
    }

    let through_own_engine = matches!(relation, CliRelation::Top)
        && crate::http::has_origin_token()
        && caller_workspace
            .as_deref()
            .is_some_and(|caller| caller != target_basename);

    match relation {
        CliRelation::Child => {
            if let Some(t) = caller_thread_id {
                obj.insert("parent_thread_id".into(), t.into());
            }
            if let Some(e) = caller_event_id {
                obj.insert("spawning_event_id".into(), e.into());
            }
            if let Some(ref mode) = args.wake_mode {
                obj.insert("wake_mode".into(), mode.clone().into());
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

    let target_root = match destination {
        SpawnDestination::Direct(root) => root,
        SpawnDestination::Routed(route) => {
            let ws = crate::workspace::resolve_from_env()?;
            let value = crate::routing::operation(
                &ws,
                &route,
                lucidos_gateway_stamp::workspace_operation::WorkspaceOperation::Spawn {
                    body: body.as_object().expect("spawn object").clone(),
                },
            ).map_err(|error| format!("{error}\nRequested thread: {thread_id}. Inspect it through the same gateway before retrying."))?;
            if value["thread_id"].as_str() != Some(&thread_id) {
                return Err(format!("The target acknowledged a different thread. The spawn was not retried. Requested thread: {thread_id}. Inspect it through the same gateway before retrying.").into());
            }
            let mut result = serde_json::json!({
                "title": spawn_title(args.title.as_deref(), &args.message),
                "thread_id": thread_id,
                "via": route.via,
                "to": args.to,
            });
            if let Some(start) = value.get("remote_start") {
                result["remote_start"] = start.clone();
            }
            println!("{result}");
            return Ok(());
        }
    };
    let target_ports = target_root.join(".lucidos/ports");
    let (api_port, recorded_proto) = read_ports(&target_ports)?;
    let proto_assumed = recorded_proto.is_none();
    let ports_proto = recorded_proto.unwrap_or_else(|| crate::workspace::DEFAULT_PROTO.to_string());
    let scheme = if args.insecure_http {
        "http"
    } else {
        &ports_proto
    };
    let client = crate::http::client()?;
    let through_gateway = if through_own_engine {
        post_through_own_engine(&client, &target_basename, &body)?
    } else {
        GatewaySpawn::Direct(None)
    };
    let through_gateway = match through_gateway {
        GatewaySpawn::Answer(status, text) => Some((status, text)),
        GatewaySpawn::Direct(reason) => {
            if through_own_engine {
                eprintln!(
                    "{}",
                    unverified_spawn_note(&target_basename, reason.as_deref().unwrap_or_default())
                );
            }
            None
        }
    };
    let (status, text) = match through_gateway {
        Some(answer) => answer,
        None => {
            let url = format!("{}://localhost:{}/api/v1/chat/stream", scheme, api_port);
            let start = std::time::Instant::now();
            let resp = client
                .post(&url)
                .header(crate::http::HEADER_TARGET_WORKSPACE, &target_basename)
                .json(&body)
                .send_with_connect_retry()
                .map_err(|e| {
                    let mut msg =
                        crate::http::format_request_error("POST", &url, &e, start.elapsed());
                    if proto_assumed && !args.insecure_http {
                        msg.push_str(&crate::workspace::assumed_proto_note(&target_ports));
                    }
                    msg
                })?;
            let status = resp.status();
            (status, resp.text().unwrap_or_default())
        }
    };
    if !status.is_success() {
        let off_ladder = serde_json::from_str::<serde_json::Value>(&text)
            .is_ok_and(|body| body["reason"] == crate::intelligence_ladder::OFF_LADDER_REASON);
        let ladder = off_ladder
            .then(|| {
                crate::intelligence_ladder::render_for_refusal(
                    &format!("{scheme}://localhost:{api_port}"),
                    Some(&target_basename),
                    selected_coding_agent.map(crate::CliCodingAgent::as_wire),
                )
            })
            .flatten()
            .map(|ladder| format!("\n\n{ladder}"))
            .unwrap_or_default();
        return Err(format!("spawn-thread failed: HTTP {}: {}{}", status, text, ladder).into());
    }

    let label = link_label(args.title.as_deref(), &args.message);
    println!("[{}](thread:{}/{})", label, target_basename, thread_id);
    Ok(())
}

enum GatewaySpawn {
    Answer(reqwest::StatusCode, String),
    Direct(Option<String>),
}

fn post_through_own_engine(
    client: &reqwest::blocking::Client,
    target: &str,
    body: &serde_json::Value,
) -> Result<GatewaySpawn, BoxError> {
    let Some(path) = lucidos_gateway_stamp::workspace_call_path(target) else {
        return Ok(GatewaySpawn::Direct(None));
    };
    let url = format!(
        "{}/api/v1{path}",
        crate::workspace::resolve_from_env()?.base_url()
    );
    let start = std::time::Instant::now();
    let resp = client
        .post(&url)
        .json(body)
        .send_with_connect_retry()
        .map_err(|e| crate::http::format_request_error("POST", &url, &e, start.elapsed()))?;
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    if hands_the_call_back(status, &text) {
        let answer: serde_json::Value = serde_json::from_str(&text)?;
        Ok(GatewaySpawn::Direct(Some(
            answer["reason"].as_str().unwrap_or_default().to_string(),
        )))
    } else {
        Ok(GatewaySpawn::Answer(status, text))
    }
}

fn unverified_spawn_note(target: &str, reason: &str) -> String {
    let cause = if reason == lucidos_gateway_stamp::REASON_WORKSPACE_NOT_ON_THIS_GATEWAY {
        String::from(
            "The target workspace is on another gateway this one is not paired with. \
                 In the target's Lucidos, open Settings → Access → Pair a gateway. \
                 Then add it as a remote gateway here.",
        )
    } else if reason == lucidos_gateway_stamp::REASON_NO_GATEWAY {
        "This engine runs without a gateway.".to_string()
    } else {
        "This call goes directly to the target, without a gateway.".to_string()
    };
    format!(
        "lucidos spawn-thread: {cause} So \"{target}\" cannot verify \
         this workspace. The thread waits for its owner to tap Allow, and starts then. \
         Nothing went wrong, so do not send it again."
    )
}

fn hands_the_call_back(status: reqwest::StatusCode, text: &str) -> bool {
    status == reqwest::StatusCode::CONFLICT
        && serde_json::from_str::<serde_json::Value>(text).is_ok_and(|answer| {
            [
                lucidos_gateway_stamp::REASON_NO_GATEWAY,
                lucidos_gateway_stamp::REASON_WORKSPACE_NOT_ON_THIS_GATEWAY,
            ]
            .iter()
            .any(|reason| answer["reason"] == *reason)
        })
}

fn check_integrated_spawn(
    relation: Option<CliRelation>,
    coding_agent: bool,
    folder: bool,
    mode: CliMode,
) -> Result<(), BoxError> {
    if !matches!(relation, Some(CliRelation::Child)) {
        return Err(
            "--integrated needs --relation child: the child folds into your own branch, \
                    so it must be your child thread."
                .into(),
        );
    }
    if !coding_agent {
        return Err(
            "--integrated needs --coding-agent, --codex or --cc: only a coding-agent \
                    child has a branch to merge."
                .into(),
        );
    }
    if folder {
        return Err(
            "--integrated needs a Lucidos-source child, and --folder targets an app. \
                    Drop --folder."
                .into(),
        );
    }
    if matches!(mode, CliMode::Human) {
        return Err(
            "--integrated does not work with --mode human: a human-mode spawn starts \
                    at once and would start independent. Drop --mode human."
                .into(),
        );
    }
    Ok(())
}

const NOTHING_TO_SEND: &str = "--message is empty and no --image was given, so there is \
    nothing to start the thread with. If the message comes from `$(cat <file>)`, check the \
    file exists.";

fn check_actor_mode(mode: CliMode, subprocess: bool) -> Result<(), BoxError> {
    if subprocess && matches!(mode, CliMode::Human) {
        return Err("--mode human cannot be used from an agent subprocess: it would record the request as the user. Omit --mode to spawn with the default agent attribution. Do not remove your origin token or post directly to the API to bypass this restriction.".into());
    }
    Ok(())
}

fn require_input(message: &str, images: &[PathBuf]) -> Result<(), BoxError> {
    if message.trim().is_empty() && images.is_empty() {
        return Err(NOTHING_TO_SEND.into());
    }
    Ok(())
}

fn inline_images(paths: &[PathBuf]) -> Result<Vec<serde_json::Value>, BoxError> {
    use base64::Engine as _;
    paths
        .iter()
        .map(|path| {
            let bytes =
                std::fs::read(path).map_err(|e| format!("--image {}: {e}", path.display()))?;
            Ok(serde_json::json!({
                "base64": base64::engine::general_purpose::STANDARD.encode(bytes),
            }))
        })
        .collect()
}

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

fn spawn_title(title: Option<&str>, message: &str) -> String {
    match title {
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
    }
}

fn link_label(title: Option<&str>, message: &str) -> String {
    spawn_title(title, message)
        .chars()
        .filter(|c| *c != ']' && *c != '\n' && *c != '\r')
        .collect()
}

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

pub(crate) fn resolve_target(name_or_path: &str) -> Result<PathBuf, BoxError> {
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
    if candidate.join(".lucidos/ports").is_file() {
        return Ok(candidate);
    }
    let missing = format!(
        "Target workspace '{}' not found at {} (no .lucidos/ports)",
        name_or_path,
        candidate.display()
    );
    let rows = crate::workspace::resolve_from_env()
        .and_then(|ws| crate::workspaces::fetch(&ws))
        .map_err(|e| format!("{missing}, and the workspace list failed: {e}"))?;
    crate::workspaces::find(&rows, name_or_path)
        .map(|row| row.path.clone())
        .ok_or_else(|| {
            format!(
                "{missing}, and this machine's workspace list does not name it. \
                 Run `lucidos workspaces list` to see the names."
            )
            .into()
        })
}

#[cfg(test)]
mod tests {
    use super::{
        check_actor_mode, check_effort_fits_model, check_integrated_spawn, effort_rows,
        inline_images, link_label, models_offering, require_input, unverified_spawn_note,
        workspaces_root, CC_MENU, CODEX_MENU, EFFORT_LEVELS, NOTHING_TO_SEND,
    };
    use crate::CliCodingAgent;
    use crate::CliMode;
    use crate::CliRelation;
    use std::collections::BTreeSet;
    use std::ffi::OsString;
    use std::path::PathBuf;

    #[test]
    fn subprocess_cannot_claim_human_attribution() {
        let error = check_actor_mode(CliMode::Human, true)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Omit --mode"));
        assert!(error.contains("Do not remove your origin token"));
        assert!(check_actor_mode(CliMode::Agent, true).is_ok());
        assert!(check_actor_mode(CliMode::Engine, true).is_ok());
    }

    #[test]
    fn terminal_actor_modes_remain_available() {
        for mode in [CliMode::Human, CliMode::Agent, CliMode::Engine] {
            assert!(check_actor_mode(mode, false).is_ok());
        }
    }

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

    #[test]
    fn an_integrated_spawn_needs_a_coding_agent_child_relation() {
        let child = Some(CliRelation::Child);
        assert!(check_integrated_spawn(child, true, false, CliMode::Agent).is_ok());
        assert!(check_integrated_spawn(child, true, false, CliMode::Engine).is_ok());
        for (relation, coding_agent, folder, mode, names) in [
            (None, true, false, CliMode::Agent, "--relation child"),
            (
                Some(CliRelation::Top),
                true,
                false,
                CliMode::Agent,
                "--relation child",
            ),
            (child, false, false, CliMode::Agent, "--coding-agent"),
            (child, true, true, CliMode::Agent, "--folder"),
            (child, true, false, CliMode::Human, "--mode human"),
        ] {
            let refusal = check_integrated_spawn(relation, coding_agent, folder, mode)
                .expect_err("refused")
                .to_string();
            assert!(refusal.contains(names), "{refusal}");
            assert!(!refusal.contains("  "), "a run of spaces in: {refusal:?}");
        }
    }

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

    #[test]
    fn only_codex_max_names_the_models_that_offer_it() {
        let codex_max = models_offering(CODEX_MENU, "max").expect("codex max names its models");
        assert!(
            codex_max
                .iter()
                .all(|m| m.starts_with("gpt-5.6") || m.starts_with("gpt-6")),
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

    #[test]
    fn home_workspaces_is_still_the_last_resort() {
        assert_eq!(
            workspaces_root(None, None, Some(PathBuf::from("/h"))),
            Some(PathBuf::from("/h/workspaces"))
        );
    }

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

    #[test]
    fn an_image_is_sent_as_its_raw_bytes_with_no_format_claim() {
        use base64::Engine as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.tiff");
        std::fs::write(&path, b"any bytes at all").unwrap();

        let entries = inline_images(&[path]).unwrap();

        assert_eq!(entries.len(), 1);
        let sent = base64::engine::general_purpose::STANDARD
            .decode(entries[0]["base64"].as_str().unwrap())
            .unwrap();
        assert_eq!(sent, b"any bytes at all");
        assert!(
            entries[0].get("mime_type").is_none(),
            "the engine sniffs the format, so the CLI claims none"
        );
    }

    #[test]
    fn a_blank_message_with_no_image_is_refused() {
        for blank in ["", " \n\t"] {
            let err = require_input(blank, &[]).unwrap_err().to_string();
            assert_eq!(err, NOTHING_TO_SEND);
        }
    }

    #[test]
    fn text_or_an_image_is_enough() {
        assert!(require_input("do the thing", &[]).is_ok());
        assert!(require_input("", &[PathBuf::from("shot.png")]).is_ok());
    }

    #[test]
    fn a_missing_image_fails_before_any_request_and_names_the_path() {
        let missing = PathBuf::from("/nonexistent/shot.png");
        let err = inline_images(&[missing]).unwrap_err().to_string();
        assert!(err.contains("--image /nonexistent/shot.png"), "{err}");
    }

    #[test]
    fn an_unverified_spawn_says_it_waits_for_the_owner() {
        let note = unverified_spawn_note("dev", lucidos_gateway_stamp::REASON_NO_GATEWAY);
        assert!(note.contains("\"dev\" cannot verify"), "{note}");
        assert!(note.contains("waits for its owner to tap Allow,"), "{note}");
        assert!(!note.contains("Allow once"), "{note}");
        assert!(note.contains("runs without a gateway"), "{note}");
        let unpaired = unverified_spawn_note(
            "dev",
            lucidos_gateway_stamp::REASON_WORKSPACE_NOT_ON_THIS_GATEWAY,
        );
        assert!(
            unpaired.contains("Settings → Access → Pair a gateway"),
            "{unpaired}"
        );
        assert!(unpaired.contains("remote gateway here"), "{unpaired}");
        let direct = unverified_spawn_note("my studio", "");
        assert!(direct.contains("directly to the target"), "{direct}");
        assert!(
            direct.contains("waits for its owner to tap Allow,"),
            "{direct}"
        );
        assert!(direct.contains("do not send it again"), "{direct}");
        assert!(note.contains("do not send it again"), "{note}");
        assert!(!note.to_lowercase().contains("failed"), "{note}");
    }
}
