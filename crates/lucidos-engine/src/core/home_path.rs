//! Home-directory path abbreviation and expansion — the two directions of the
//! same `~` ⇄ `$HOME` mapping, in one place.
//!
//! **Why abbreviation exists.** Absolute paths the engine renders into
//! LLM-visible text ship to a third-party model provider on every turn, and a
//! home directory name is not neutral: MDM-managed corporate fleets routinely
//! name it `<username>@<employer-domain>`, so a bare
//! `WORKSPACE: myws (/Users/…/workspaces/myws)` line hands the provider
//! the user's employer on every request. [`abbreviate`] collapses the `$HOME`
//! prefix to `~` so prompt text carries the shape of the path without the
//! identity baked into it. See `.claude/rules/no-private-data.md`.
//!
//! **Abbreviation is for text the model reads, never for a path the engine
//! uses.** Anything handed to a tool, a subprocess, a git worktree, or the
//! filesystem keeps its real absolute form — `~` is not expanded by
//! `std::fs`, and Python's `open()` will not expand it either. Abbreviate at
//! the point of rendering, nowhere earlier.
//!
//! [`expand`] is the inverse, for user- or model-supplied `~/…` input that has
//! to become a real path again (the repository registry, coding-agent `folder`
//! resolution).

use std::path::Path;

/// Collapse a `$HOME`-rooted prefix to `~` for LLM-visible text. Returns the
/// path unchanged when it is not under `$HOME` (or `$HOME` is unset).
pub fn abbreviate(path: &Path) -> String {
    abbreviate_with(path, home().as_deref())
}

/// [`abbreviate`] for a path already held as a string (DB columns, JSON
/// values) — same rules, no `PathBuf` round-trip at the call site.
pub fn abbreviate_str(path: &str) -> String {
    abbreviate(Path::new(path))
}

/// Expand a leading `~` / `~/…` to `$HOME`. Returns the input unchanged when
/// it has no tilde prefix (or `$HOME` is unset).
pub fn expand(path: &str) -> String {
    expand_with(path, home().as_deref())
}

fn home() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from)
}

/// Pure core of [`abbreviate`] — `home` injected so it is testable without
/// touching process env.
///
/// Strips only a WHOLE-COMPONENT prefix (via `Path::strip_prefix`), so a
/// sibling directory that merely shares the home dir's textual prefix
/// (`<home>-backup` next to `<home>`) is left alone. A plain
/// `str::strip_prefix` would mangle it into `~-backup`.
fn abbreviate_with(path: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.display().to_string();
    };
    match path.strip_prefix(home) {
        // The home dir itself.
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Pure core of [`expand`] — `home` injected (see [`abbreviate_with`]).
fn expand_with(path: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return path.to_string();
    };
    if path == "~" {
        return home.display().to_string();
    }
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest).display().to_string(),
        None => path.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{abbreviate_with, expand_with};
    use std::path::Path;

    // A synthetic home root, not a `/Users/<name>` literal. The prefix cases
    // below need sibling dirs whose NAMES extend the home dir's name
    // (`<home>2`, `<home>xyz`) — spelling those under `/Users/` would plant
    // exactly the real-home-path shape the repo's private-data guard exists to
    // catch. The function is root-agnostic, so a neutral root tests it just as
    // well. See `.claude/rules/no-private-data.md`.
    const HOME: &str = "/opt/testhome";

    fn home() -> Option<&'static Path> {
        Some(Path::new(HOME))
    }

    #[test]
    fn abbreviates_a_home_rooted_path() {
        assert_eq!(
            abbreviate_with(Path::new("/opt/testhome/workspaces/myws"), home()),
            "~/workspaces/myws"
        );
    }

    #[test]
    fn abbreviates_the_home_dir_itself() {
        assert_eq!(abbreviate_with(Path::new(HOME), home()), "~");
    }

    #[test]
    fn leaves_a_sibling_sharing_the_home_prefix_alone() {
        // A plain textual strip would produce "~-backup" / "~2" — the
        // whole-component strip must not treat these as living under $HOME.
        for p in [
            "/opt/testhome-backup/x",
            "/opt/testhome2",
            "/opt/testhomexyz/notes.md",
        ] {
            assert_eq!(abbreviate_with(Path::new(p), home()), p);
        }
    }

    #[test]
    fn leaves_non_home_absolute_and_relative_paths_alone() {
        for p in ["/opt/homebrew/bin/git", "/tmp/x", "data/artifacts/notes.md"] {
            assert_eq!(abbreviate_with(Path::new(p), home()), p);
        }
    }

    #[test]
    fn without_home_the_path_passes_through() {
        assert_eq!(
            abbreviate_with(Path::new("/opt/testhome/ws"), None),
            "/opt/testhome/ws"
        );
    }

    #[test]
    fn expands_tilde_forms() {
        assert_eq!(expand_with("~", home()), HOME);
        assert_eq!(expand_with("~/repos/x", home()), "/opt/testhome/repos/x");
    }

    #[test]
    fn leaves_non_tilde_input_alone() {
        // A bare `~name` is another user's home in shell syntax, which we do
        // not resolve — and `~backup` must not become `$HOME/backup`.
        for p in ["/opt/testhome/x", "repos/x", "~backup", ""] {
            assert_eq!(expand_with(p, home()), p);
        }
    }

    #[test]
    fn expand_without_home_passes_through() {
        assert_eq!(expand_with("~/repos/x", None), "~/repos/x");
    }

    #[test]
    fn abbreviate_and_expand_round_trip() {
        let abs = "/opt/testhome/workspaces/dev";
        let short = abbreviate_with(Path::new(abs), home());
        assert_eq!(short, "~/workspaces/dev");
        assert_eq!(expand_with(&short, home()), abs);
    }
}
