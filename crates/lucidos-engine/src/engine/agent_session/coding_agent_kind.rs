//! `CodingAgentKind` — the thread-flavor discriminator for coding-agent threads.
//!
//! Every coding-agent thread lives in a worktree, but the *enclosing git*
//! differs by kind:
//!
//! - `Lucidos` — full worktree of the Lucidos source repo. Apply ff-merges to
//!   main; `/harden` gate; engine restart per `files_require_restart`.
//! - `App` — sparse-checkout worktree of the user's workspace git, narrowed
//!   to `data/apps/<id>/`. Apply ff-merges to workspace main; no `/harden`;
//!   no engine restart; emits `AppUiRefreshRequested` when iframe-bundled
//!   files change.
//! - `External` — full worktree of a user-registered external git repository.
//!   No Apply gate (PR-based workflow).
//!
//! Distinct from `crate::runtime::CodingAgent`, which discriminates the
//! backend (today only `ClaudeCode`). A `CodingAgent::ClaudeCode` session can
//! be of any `CodingAgentKind`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The kind of coding-agent thread. Serialized on `SessionStarted` events and
/// stored in `thread_summaries.coding_agent_kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentKind {
    /// Edits Lucidos itself (the engine source tree). Default for legacy rows
    /// where no kind was recorded.
    #[default]
    Lucidos,
    /// Edits a single app folder at `data/apps/<id>/` in the user's workspace.
    App,
    /// Edits a user-registered external git repository.
    External,
}

impl CodingAgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lucidos => "lucidos",
            Self::App => "app",
            Self::External => "external",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "app" => Self::App,
            "external" => Self::External,
            _ => Self::Lucidos,
        }
    }
}

/// Shorthand for the boxed-trait error this module surfaces. Per
/// `.claude/rules/rust.md`, custom error enums need structural justification
/// (callers must pattern-match on variants); resolve/classify errors here
/// are only ever string-formatted by `agentic_loop_special_tool`, so the
/// canonical boxed form keeps us out of bespoke-error-type land. Helper
/// constructors below produce the same teaching strings the old enum's
/// `Display` impl rendered.
pub type FolderResolutionError = Box<dyn std::error::Error + Send + Sync>;

// Every path in the err_* messages below is rendered with
// `core::home_path::abbreviate`. They are returned verbatim to the chat model
// (`agentic_loop_special_tool` formats the error straight into the
// `run_coding_agent` tool result), and a home dir named
// `<username>@<employer-domain>` would ship to the provider on every failed
// resolve. `~/…` still tells the agent exactly which folder it named, and
// `resolve_folder_input` expands `~` on the way back in.

fn err_not_found(path: &str) -> FolderResolutionError {
    format!(
        "Folder does not exist: {path}. \
         Pass an absolute path, a workspace-relative path \
         (`data/apps/<id>`), or a registered repo name from \
         `manage_repositories`."
    )
    .into()
}

fn err_inside_lucidos_state(path: &Path) -> FolderResolutionError {
    format!(
        "Folder `{}` is inside the engine's `.lucidos/` state directory \
         and cannot be edited by a coding-agent thread. Pick a real source \
         folder.",
        crate::core::home_path::abbreviate(path)
    )
    .into()
}

fn err_forbidden_system_path(path: &Path) -> FolderResolutionError {
    format!(
        "Folder `{}` is a system path and cannot be edited by a coding-agent thread.",
        crate::core::home_path::abbreviate(path)
    )
    .into()
}

fn err_non_app_data_path(path: &Path, workspace_root: &Path) -> FolderResolutionError {
    format!(
        "Folder `{}` is under `{}/data/` but is not exactly \
         `data/apps/<app_id>/`. Coding-agent threads on workspace data are \
         only supported for whole app folders. For knowhow / triggers / \
         artifacts / config / scripts, use the chat path: file tools + the \
         `lucidos` CLI.",
        crate::core::home_path::abbreviate(path),
        crate::core::home_path::abbreviate(workspace_root)
    )
    .into()
}

fn err_unrecognised_path(path: &Path, workspace_root: &Path) -> FolderResolutionError {
    format!(
        "Folder `{}` is not the Lucidos source repo, not under \
         `{}/data/apps/`, and not a registered external repo. Coding-agent \
         threads on unregistered folders or non-git directories are not \
         supported in v1. Register the repo with \
         `manage_repositories action='add'` if it's a git repo, otherwise \
         use the chat path.",
        crate::core::home_path::abbreviate(path),
        crate::core::home_path::abbreviate(workspace_root)
    )
    .into()
}

fn err_repo_lookup(msg: String) -> FolderResolutionError {
    format!("Repository lookup failed: {msg}").into()
}

/// Refusal for a Lucidos-**source** coding-agent spawn on an install that was
/// not launched from a source checkout (see
/// [`crate::paths::has_lucidos_source`]).
///
/// Two call sites share this one string so the model gets identical, actionable
/// wording wherever it asks: the synchronous `run_coding_agent` refusal in
/// `agentic_loop_special_tool` (which returns it as the tool result, in-turn),
/// and the `run_session` guard that backstops every other caller.
///
/// Actionable matters here. The failure this replaces was the agent *inventing*
/// a capability, narrating edits to `crates/lucidos-engine/…`, and telling the
/// user to Apply and restart — so the message must both deny the request and
/// name the routes that DO work, or the model will simply retry the same spawn.
pub fn err_no_lucidos_source() -> FolderResolutionError {
    "This Lucidos install has no source checkout — the engine was not launched \
     from a Lucidos source tree, so there is no platform source here to edit and \
     no coding-agent thread can be started against it ON THIS INSTALL. Do NOT \
     claim you can change Lucidos itself here, and do not tell the user to Apply \
     or restart for such a change. What DOES work: `folder=\"data/apps/<id>\"` to \
     edit an installed app, or `folder=<repo name>` for a repository registered \
     via `manage_repositories`. Changing Lucidos itself requires an engine \
     running from a Lucidos source checkout (a dev build) — if the user has such \
     a workspace, route the work there with \
     `run_coding_agent(workspace=\"<name>\", relation=\"top\")`, which is \
     forwarded to that engine and is NOT blocked by this refusal."
        .into()
}

/// Forbidden system roots. Lookups are anchored at start so a `/etcetera`
/// folder isn't accidentally captured.
const FORBIDDEN_SYSTEM_ROOTS: &[&str] = &[
    "/etc", "/System", "/usr", "/bin", "/sbin", "/var", "/private", "/dev", "/Library",
];

/// True if `path_under_data` matches `apps/<id>/` exactly (no subpath after).
/// Returns the app id when matched.
fn match_app_id(path_under_data: &Path) -> Option<String> {
    let mut comps = path_under_data.components();
    let first = comps.next()?.as_os_str().to_str()?;
    if first != "apps" {
        return None;
    }
    let id = comps.next()?.as_os_str().to_str()?;
    if id.is_empty() || id == "." || id == ".." {
        return None;
    }
    // Refuse deeper paths inside the app — scope unit is the whole app folder.
    if comps.next().is_some() {
        return None;
    }
    Some(id.to_string())
}

/// True if `id` is a syntactically valid app id (no path traversal, no slashes).
pub fn is_valid_app_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains("..")
        && !id.contains('/')
        && !id.contains('\\')
        && !id.starts_with('.')
}

/// Resolve a user/LLM-supplied `folder` input string to a canonical absolute
/// path, applying every refusal rule in §3.2 of the design plan.
///
/// `folder_in` is the raw `folder` parameter (or its `repo` alias) from
/// `run_coding_agent`. `workspace_root` is the target workspace's root directory.
/// `lookup_repo_path` resolves a registered repo name or UUID to its path
/// (typically wraps `RepositoryStore::get` + `get_by_name`).
///
/// Returns the canonicalized absolute path on success.
pub fn resolve_folder_input<F>(
    folder_in: &str,
    workspace_root: &Path,
    lookup_repo_path: F,
) -> Result<PathBuf, FolderResolutionError>
where
    F: FnOnce(&str) -> Result<Option<PathBuf>, String>,
{
    let trimmed = folder_in.trim();
    if trimmed.is_empty() {
        return Err(err_not_found(trimmed));
    }
    // `~/…` → absolute, before anything classifies it. LLM-visible listings
    // (`manage_repositories` action=list) render repo paths abbreviated so the
    // home dir's name never reaches the model provider, so the model naturally
    // echoes the `~` form back here. `looks_like_repo_name` already rejects
    // `~`, and expansion is a no-op for every other shape.
    let expanded = crate::core::home_path::expand(trimmed);
    let trimmed = expanded.as_str();
    let candidate: PathBuf = if trimmed.starts_with('/') {
        PathBuf::from(trimmed)
    } else if looks_like_repo_name(trimmed) {
        match lookup_repo_path(trimmed).map_err(err_repo_lookup)? {
            Some(p) => p,
            None => return Err(err_not_found(trimmed)),
        }
    } else {
        workspace_root.join(trimmed)
    };

    let canonical = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());

    if !canonical.exists() {
        let shown = crate::core::home_path::abbreviate(&candidate);
        return Err(err_not_found(&shown));
    }
    Ok(canonical)
}

/// Heuristic: does `s` look like a registered repo name/UUID rather than a
/// filesystem path? Rejects anything containing `/`, `\`, `~`, or `.` — those
/// are clearly path-shaped. Also rejects strings starting with `data/` for
/// safety (workspace-relative paths often start with `data/`).
fn looks_like_repo_name(s: &str) -> bool {
    !s.contains('/') && !s.contains('\\') && !s.contains('~') && !s.contains('.') && !s.is_empty()
}

/// Outcome of [`classify_resolved_folder`]: which `CodingAgentKind` the path
/// maps to, plus the deduced app id when applicable.
pub enum FolderClassification {
    Lucidos {
        repo_root: PathBuf,
    },
    App {
        workspace_root: PathBuf,
        app_id: String,
    },
    External {
        repo_root: PathBuf,
    },
}

/// Apply the kind-selection + refusal rules to an already-canonicalized
/// `folder_abs` path. Mirrors §3.2 step 3 of the design plan.
///
/// `lucidos_repo_root` is the resolved path of the Lucidos source repo
/// (from `RepositoryStore::get_by_name("Lucidos")`), or `None` when there is no
/// source checkout — a packaged build, where "Lucidos source" is not a thing.
/// `None` means the Lucidos-source branch simply never matches (a would-be
/// source path falls through to `UnrecognisedPath`); App + External still
/// classify. `workspace_root` is the target workspace's root.
/// `external_repo_match` returns the canonical registered-external-repo root if
/// `folder_abs` equals one of them.
pub fn classify_resolved_folder<F>(
    folder_abs: &Path,
    workspace_root: &Path,
    lucidos_repo_root: Option<&Path>,
    external_repo_match: F,
) -> Result<FolderClassification, FolderResolutionError>
where
    F: FnOnce(&Path) -> Option<PathBuf>,
{
    let ws_lucidos_state = workspace_root.join(".lucidos");
    if folder_abs.starts_with(&ws_lucidos_state) {
        return Err(err_inside_lucidos_state(folder_abs));
    }
    // External-repo registry trumps the system-root check — a user who
    // explicitly registered `/var/code/myproj` or `/usr/local/src/proj` (or
    // any canonicalized path under `/private/var/folders/...` on macOS)
    // chose to make it a CC target. Skip the system-root refusal when the
    // registry already vouches for the path. Done up front so the External
    // branch below can simply re-call the closure without worrying about
    // having been pre-empted by ForbiddenSystemPath.
    let external_repo_root_for_path = external_repo_match(folder_abs);
    if external_repo_root_for_path.is_none() {
        // System-root refusal only applies to paths outside the workspace and
        // outside the Lucidos source repo — workspaces are user-positioned
        // and can legitimately live under `/var/folders/T/...` on macOS or
        // any other system-shaped path the user picked.
        if !folder_abs.starts_with(workspace_root)
            && !lucidos_repo_root.is_some_and(|root| folder_abs.starts_with(root))
        {
            let folder_str = folder_abs.to_string_lossy();
            for root in FORBIDDEN_SYSTEM_ROOTS {
                if folder_str == *root || folder_str.starts_with(&format!("{root}/")) {
                    return Err(err_forbidden_system_path(folder_abs));
                }
            }
        }
    }

    let ws_data = workspace_root.join("data");
    if folder_abs.starts_with(&ws_data) {
        let under_data = folder_abs.strip_prefix(&ws_data).unwrap_or(folder_abs);
        if let Some(app_id) = match_app_id(under_data) {
            if !is_valid_app_id(&app_id) {
                return Err(err_non_app_data_path(folder_abs, workspace_root));
            }
            return Ok(FolderClassification::App {
                workspace_root: workspace_root.to_path_buf(),
                app_id,
            });
        }
        // Under data/ — non-apps subtree or bare `data/`. All refused.
        return Err(err_non_app_data_path(folder_abs, workspace_root));
    }

    // Lucidos source repo: exact-match or descendant. Skipped entirely when no
    // source checkout exists (packaged) — the branch can't match, so a would-be
    // source path falls through to the UnrecognisedPath refusal below.
    if let Some(lucidos_repo_root) = lucidos_repo_root {
        if folder_abs == lucidos_repo_root || folder_abs.starts_with(lucidos_repo_root) {
            return Ok(FolderClassification::Lucidos {
                repo_root: lucidos_repo_root.to_path_buf(),
            });
        }
    }

    // External repo registry: only exact-match counts (registered roots are
    // the unit of registration; subpaths into them are out of scope). We
    // already consulted the registry above to decide whether to skip the
    // system-root refusal — reuse that result rather than calling the
    // FnOnce closure twice.
    if let Some(ext_root) = external_repo_root_for_path {
        return Ok(FolderClassification::External {
            repo_root: ext_root,
        });
    }

    Err(err_unrecognised_path(folder_abs, workspace_root))
}

// Small helper so the FolderClassification debug-prints land in test panics
// without needing to derive Debug on the public enum.
impl std::fmt::Debug for FolderClassification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lucidos { repo_root } => f
                .debug_struct("Lucidos")
                .field("repo_root", repo_root)
                .finish(),
            Self::App {
                workspace_root,
                app_id,
            } => f
                .debug_struct("App")
                .field("workspace_root", workspace_root)
                .field("app_id", app_id)
                .finish(),
            Self::External { repo_root } => f
                .debug_struct("External")
                .field("repo_root", repo_root)
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `manage_repositories` action=`list` renders repo paths as `~/…` so the
    /// home dir's name (on an MDM fleet: `<username>@<employer-domain>`) never
    /// reaches the model provider. That only works if the abbreviated form the
    /// model echoes back as `folder` still resolves — otherwise the privacy fix
    /// would break `run_coding_agent` against every registered repo.
    #[test]
    fn resolve_folder_input_expands_a_tilde_path() {
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let expected = std::fs::canonicalize(&home).expect("home dir must exist");
        let workspace_root = Path::new("/nonexistent-workspace-root");

        let resolved = resolve_folder_input("~", workspace_root, |_| Ok(None))
            .expect("`~` must resolve to the home dir");
        assert_eq!(resolved, expected);

        // Trailing-segment form goes through the same expansion — `~/.` is the
        // home dir again, and is guaranteed to exist on any machine.
        let resolved = resolve_folder_input("~/.", workspace_root, |_| Ok(None))
            .expect("`~/.` must resolve to the home dir");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_folder_input_still_rejects_a_missing_tilde_path() {
        // Expansion must not turn a bogus path into a silent workspace-relative
        // join — a `~/…` that doesn't exist is still "not found".
        let err = resolve_folder_input(
            "~/definitely-not-a-real-directory-9f3a2b",
            Path::new("/nonexistent-workspace-root"),
            |_| Ok(None),
        );
        assert!(err.is_err());
    }

    #[test]
    fn coding_agent_kind_round_trips_str() {
        for k in [
            CodingAgentKind::Lucidos,
            CodingAgentKind::App,
            CodingAgentKind::External,
        ] {
            assert_eq!(CodingAgentKind::parse(k.as_str()), k);
        }
    }

    #[test]
    fn coding_agent_kind_parses_unknown_to_lucidos() {
        assert_eq!(CodingAgentKind::parse(""), CodingAgentKind::Lucidos);
        assert_eq!(CodingAgentKind::parse("nonsense"), CodingAgentKind::Lucidos);
    }

    #[test]
    fn match_app_id_accepts_exact_app_folder() {
        assert_eq!(
            match_app_id(Path::new("apps/habit-tracker")),
            Some("habit-tracker".to_string())
        );
        // Trailing slash gets normalized away by Path components.
        assert_eq!(
            match_app_id(Path::new("apps/habit-tracker/")),
            Some("habit-tracker".to_string())
        );
    }

    #[test]
    fn match_app_id_refuses_subpaths() {
        assert_eq!(match_app_id(Path::new("apps/habit-tracker/ui")), None);
        assert_eq!(
            match_app_id(Path::new("apps/habit-tracker/index.html")),
            None
        );
        assert_eq!(
            match_app_id(Path::new("apps/habit-tracker/knowhow/intent.md")),
            None
        );
    }

    #[test]
    fn match_app_id_refuses_non_apps_subtrees() {
        assert_eq!(match_app_id(Path::new("knowhow")), None);
        assert_eq!(match_app_id(Path::new("triggers/morning")), None);
        assert_eq!(match_app_id(Path::new("artifacts/foo.md")), None);
    }

    #[test]
    fn match_app_id_refuses_path_traversal_in_id() {
        assert_eq!(match_app_id(Path::new("apps/.")), None);
        assert_eq!(match_app_id(Path::new("apps/..")), None);
        assert_eq!(match_app_id(Path::new("apps/")), None);
    }

    #[test]
    fn is_valid_app_id_rejects_traversal() {
        assert!(!is_valid_app_id(""));
        assert!(!is_valid_app_id("."));
        assert!(!is_valid_app_id(".."));
        assert!(!is_valid_app_id("a/b"));
        assert!(!is_valid_app_id("a\\b"));
        assert!(!is_valid_app_id(".hidden"));
        assert!(is_valid_app_id("habit-tracker"));
        assert!(is_valid_app_id("habit-tracker"));
        assert!(is_valid_app_id("app_v2"));
    }

    #[test]
    fn looks_like_repo_name_rules() {
        assert!(looks_like_repo_name("acme"));
        assert!(looks_like_repo_name("abc123def-456"));
        assert!(!looks_like_repo_name("data/apps/foo"));
        assert!(!looks_like_repo_name("./repo"));
        assert!(!looks_like_repo_name("~/repos/x"));
        assert!(!looks_like_repo_name("a.b"));
        assert!(!looks_like_repo_name(""));
    }

    #[test]
    fn classify_refuses_inside_lucidos_state() {
        let ws = std::env::temp_dir().join("classify_test_ws_state");
        let target = ws.join(".lucidos/worktrees/thread-foo");
        let _ = std::fs::create_dir_all(&target);
        let lucidos_root = ws.join("lucidos-source");
        let _ = std::fs::create_dir_all(&lucidos_root);
        let res = classify_resolved_folder(&target, &ws, Some(&lucidos_root), |_| None);
        let err = res.expect_err("expected refusal").to_string();
        assert!(
            err.contains(".lucidos/"),
            "error must explain the .lucidos/ state refusal: {err}",
        );
    }

    #[test]
    fn classify_returns_app_for_canonical_app_folder() {
        let ws = std::env::temp_dir().join("classify_test_ws_app");
        let target = ws.join("data/apps/habit-tracker");
        let _ = std::fs::create_dir_all(&target);
        let lucidos_root = ws.join("lucidos-source");
        let _ = std::fs::create_dir_all(&lucidos_root);
        let res = classify_resolved_folder(&target, &ws, Some(&lucidos_root), |_| None);
        match res {
            Ok(FolderClassification::App {
                workspace_root,
                app_id,
            }) => {
                assert_eq!(workspace_root, ws);
                assert_eq!(app_id, "habit-tracker");
            }
            other => panic!(
                "expected App, got {:?}",
                other.as_ref().err().map(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn classify_refuses_app_subpath() {
        let ws = std::env::temp_dir().join("classify_test_ws_appsub");
        let target = ws.join("data/apps/habit-tracker/ui");
        let _ = std::fs::create_dir_all(&target);
        let lucidos_root = ws.join("lucidos-source");
        let _ = std::fs::create_dir_all(&lucidos_root);
        let res = classify_resolved_folder(&target, &ws, Some(&lucidos_root), |_| None);
        let err = res.expect_err("expected refusal").to_string();
        assert!(
            err.contains("data/apps/<app_id>"),
            "error must point at the canonical app-folder shape: {err}",
        );
    }

    #[test]
    fn classify_refuses_non_app_data_subtree() {
        let ws = std::env::temp_dir().join("classify_test_ws_data_knowhow");
        let target = ws.join("data/knowhow");
        let _ = std::fs::create_dir_all(&target);
        let lucidos_root = ws.join("lucidos-source");
        let _ = std::fs::create_dir_all(&lucidos_root);
        let res = classify_resolved_folder(&target, &ws, Some(&lucidos_root), |_| None);
        let err = res.expect_err("expected refusal").to_string();
        assert!(
            err.contains("`lucidos` CLI"),
            "error must point at the chat-path / lucidos-CLI alternative: {err}",
        );
    }

    #[test]
    fn classify_returns_lucidos_for_repo_root() {
        let ws = std::env::temp_dir().join("classify_test_ws_lucidos");
        let lucidos_root = ws.join("lucidos");
        let _ = std::fs::create_dir_all(&lucidos_root);
        let res = classify_resolved_folder(&lucidos_root, &ws, Some(&lucidos_root), |_| None);
        match res {
            Ok(FolderClassification::Lucidos { repo_root }) => {
                assert_eq!(repo_root, lucidos_root);
            }
            other => panic!(
                "expected Lucidos, got {:?}",
                other.as_ref().err().map(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn classify_returns_external_when_registry_matches() {
        let ws = std::env::temp_dir().join("classify_test_ws_external");
        let lucidos_root = ws.join("lucidos");
        let _ = std::fs::create_dir_all(&lucidos_root);
        let ext = ws.join("acme");
        let _ = std::fs::create_dir_all(&ext);
        let ext_ref = ext.clone();
        let res = classify_resolved_folder(&ext, &ws, Some(&lucidos_root), |p| {
            if p == ext_ref {
                Some(ext_ref.clone())
            } else {
                None
            }
        });
        match res {
            Ok(FolderClassification::External { repo_root }) => assert_eq!(repo_root, ext),
            other => panic!(
                "expected External, got {:?}",
                other.as_ref().err().map(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn classify_refuses_unrecognised_path() {
        let ws = std::env::temp_dir().join("classify_test_ws_unknown");
        let target = ws.join("scratch");
        let _ = std::fs::create_dir_all(&target);
        let lucidos_root = ws.join("lucidos");
        let _ = std::fs::create_dir_all(&lucidos_root);
        let res = classify_resolved_folder(&target, &ws, Some(&lucidos_root), |_| None);
        let err = res.expect_err("expected refusal").to_string();
        assert!(
            err.contains("not the Lucidos source repo"),
            "error must explain why the path was rejected: {err}",
        );
    }

    // --- Packaged build: no Lucidos source repo registered (lucidos_repo_root = None) ---

    #[test]
    fn classify_app_without_lucidos_source() {
        // Packaged: no source checkout, yet an app folder must still classify so
        // app coding-agent spawns work (the regression this fix targets).
        let ws = std::env::temp_dir().join("classify_test_ws_app_no_src");
        let target = ws.join("data/apps/habit-tracker");
        let _ = std::fs::create_dir_all(&target);
        let res = classify_resolved_folder(&target, &ws, None, |_| None);
        match res {
            Ok(FolderClassification::App {
                workspace_root,
                app_id,
            }) => {
                assert_eq!(workspace_root, ws);
                assert_eq!(app_id, "habit-tracker");
            }
            other => panic!(
                "expected App, got {:?}",
                other.as_ref().err().map(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn classify_external_without_lucidos_source() {
        // Packaged: a registered external repo must still classify with no source.
        let ws = std::env::temp_dir().join("classify_test_ws_ext_no_src");
        let ext = ws.join("acme");
        let _ = std::fs::create_dir_all(&ext);
        let ext_ref = ext.clone();
        let res = classify_resolved_folder(&ext, &ws, None, |p| {
            if p == ext_ref {
                Some(ext_ref.clone())
            } else {
                None
            }
        });
        match res {
            Ok(FolderClassification::External { repo_root }) => assert_eq!(repo_root, ext),
            other => panic!(
                "expected External, got {:?}",
                other.as_ref().err().map(|e| e.to_string())
            ),
        }
    }

    #[test]
    fn classify_would_be_source_falls_through_without_lucidos_source() {
        // Packaged: a path that WOULD be the Lucidos source on a dev build must
        // NOT be silently classified — with no source it's UnrecognisedPath.
        let ws = std::env::temp_dir().join("classify_test_ws_src_falls_through");
        let would_be_source = ws.join("lucidos");
        let _ = std::fs::create_dir_all(&would_be_source);
        let res = classify_resolved_folder(&would_be_source, &ws, None, |_| None);
        let err = res
            .expect_err("expected refusal with no Lucidos source")
            .to_string();
        assert!(
            err.contains("not the Lucidos source repo"),
            "would-be source must fall through to the unrecognised-path refusal: {err}",
        );
    }

    /// The refusal both spawn sites share must be ACTIONABLE, not just a denial.
    /// The observed failure was the agent narrating edits to
    /// `crates/lucidos-engine/…` and telling the user to Apply + restart on a
    /// packaged install; a bare "no" invites the model to retry the same spawn,
    /// so the message has to name the routes that actually work.
    #[test]
    fn no_lucidos_source_refusal_names_the_working_alternatives() {
        let msg = err_no_lucidos_source().to_string();
        assert!(
            msg.contains("no source checkout"),
            "must state WHY it is refused: {msg}"
        );
        assert!(
            msg.contains("data/apps/<id>"),
            "must point at the app coding-agent route: {msg}"
        );
        assert!(
            msg.contains("manage_repositories"),
            "must point at the external-repo route: {msg}"
        );
        assert!(
            msg.contains("Apply"),
            "must warn off the apply/restart narration that made this a user-visible lie: {msg}"
        );
        // Scoped to this install: a cross-workspace spawn returns before this
        // guard and is enforced by the target engine, so the refusal must not
        // read as "no coding agent can ever do platform work".
        assert!(
            msg.contains("ON THIS INSTALL") && msg.contains("run_coding_agent(workspace="),
            "refusal must scope itself and keep the cross-workspace route open: {msg}"
        );
    }
}
