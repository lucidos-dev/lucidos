use std::path::{Path, PathBuf};

pub type FolderResolutionError = Box<dyn std::error::Error + Send + Sync>;

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

pub fn err_no_lucidos_source() -> FolderResolutionError {
    "This Lucidos install has no source checkout — the engine was not launched \
     from a Lucidos source tree, so there is no platform source here to edit and \
     no coding-agent thread can be started against it ON THIS INSTALL. Do NOT \
     claim you can change Lucidos itself here, and do not tell the user to Apply \
     or restart for such a change. What DOES work: `folder=\"data/apps/<id>\"` to \
     edit an installed app, or `folder=<repo name>` for a repository registered \
     via `manage_repositories`. Changing Lucidos itself requires an engine \
     running from a Lucidos source checkout (a dev build). That is another \
     install, which `run_coding_agent(workspace=…)` cannot reach. If the user \
     has such a workspace, route the work there with `run_bash`: `lucidos \
     spawn-thread --to <absolute workspace path> --coding-agent \
     claude-code|codex --message '<task>'`. This refusal does not block it."
        .into()
}

const FORBIDDEN_SYSTEM_ROOTS: &[&str] = &[
    "/etc", "/System", "/usr", "/bin", "/sbin", "/var", "/private", "/dev", "/Library",
];

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
    if comps.next().is_some() {
        return None;
    }
    Some(id.to_string())
}

use crate::core::apps::is_valid_app_id;

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

fn looks_like_repo_name(s: &str) -> bool {
    !s.contains('/') && !s.contains('\\') && !s.contains('~') && !s.contains('.') && !s.is_empty()
}

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
    let external_repo_root_for_path = external_repo_match(folder_abs);
    if external_repo_root_for_path.is_none()
        && !folder_abs.starts_with(workspace_root)
        && !lucidos_repo_root.is_some_and(|root| folder_abs.starts_with(root))
    {
        let folder_str = folder_abs.to_string_lossy();
        for root in FORBIDDEN_SYSTEM_ROOTS {
            if folder_str == *root || folder_str.starts_with(&format!("{root}/")) {
                return Err(err_forbidden_system_path(folder_abs));
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
        return Err(err_non_app_data_path(folder_abs, workspace_root));
    }

    if let Some(lucidos_repo_root) = lucidos_repo_root {
        if folder_abs == lucidos_repo_root || folder_abs.starts_with(lucidos_repo_root) {
            return Ok(FolderClassification::Lucidos {
                repo_root: lucidos_repo_root.to_path_buf(),
            });
        }
    }

    if let Some(ext_root) = external_repo_root_for_path {
        return Ok(FolderClassification::External {
            repo_root: ext_root,
        });
    }

    Err(err_unrecognised_path(folder_abs, workspace_root))
}

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
    use crate::engine::thread_events::CodingAgentKind;

    #[test]
    fn resolve_folder_input_expands_a_tilde_path() {
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let expected = std::fs::canonicalize(&home).expect("home dir must exist");
        let workspace_root = Path::new("/nonexistent-workspace-root");

        let resolved = resolve_folder_input("~", workspace_root, |_| Ok(None))
            .expect("`~` must resolve to the home dir");
        assert_eq!(resolved, expected);

        let resolved = resolve_folder_input("~/.", workspace_root, |_| Ok(None))
            .expect("`~/.` must resolve to the home dir");
        assert_eq!(resolved, expected);
    }

    #[test]
    fn resolve_folder_input_still_rejects_a_missing_tilde_path() {
        let err = resolve_folder_input(
            "~/definitely-not-a-real-directory-9f3a2b",
            Path::new("/nonexistent-workspace-root"),
            |_| Ok(None),
        );
        assert!(err.is_err());
    }

    #[test]
    fn refusal_messages_never_carry_the_home_dir() {
        let home = std::env::var("HOME").expect("HOME must be set to run this test");
        let home_dir = Path::new(&home);
        let ws = home_dir.join("kind-test-workspace");
        let lucidos_root = home_dir.join("kind-test-source");
        let mut messages =
            vec![
                resolve_folder_input("~/definitely-not-a-real-directory-9f3a2b", &ws, |_| {
                    Ok(None)
                })
                .expect_err("a missing folder is refused")
                .to_string(),
            ];
        for folder in [
            ws.join(".lucidos/worktrees/thread-foo"),
            ws.join("data/knowhow"),
            home_dir.join("kind-test-elsewhere"),
        ] {
            messages.push(
                classify_resolved_folder(&folder, &ws, Some(&lucidos_root), |_| None)
                    .expect_err("expected refusal")
                    .to_string(),
            );
        }
        for message in messages {
            assert!(
                !message.contains(&home) && message.contains('~'),
                "a refusal reaches the model provider, so it must name the path as `~/…`: {message}"
            );
        }
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
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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

    #[test]
    fn classify_app_without_lucidos_source() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().to_path_buf();
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
        assert!(
            msg.contains("ON THIS INSTALL")
                && msg.contains("lucidos spawn-thread --to <absolute workspace path>"),
            "refusal must scope itself and name the route to another install: {msg}"
        );
    }
}
