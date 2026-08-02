//! Locating the `tailscale` CLI.
//!
//! Reading state needs no CLI (see the crate docs). This exists for the one
//! operation with no other interface: `tailscale serve`, which is what turns a
//! plain-HTTP tailnet address into `https://<machine>.<tailnet>.ts.net` and so
//! into an installable PWA with push. There is no GUI, config file, or admin
//! console equivalent, so a machine without a CLI genuinely cannot be exposed.

use std::path::Path;

/// Absolute-path candidates for the Tailscale CLI, most likely first.
///
/// **Real CLI binaries only.** `/Applications/Tailscale.app/Contents/MacOS/
/// Tailscale` is deliberately NOT here: it is the GUI executable. Outside a GUI
/// session it prints "The Tailscale GUI failed to start ... (Tailscale.CLIError
/// error 3)" to stdout and **exits 0**, so a caller that trusts the exit code
/// reads it as a success that produced unparseable output. Since resolution
/// picks ONE candidate by existence with no fall-through, listing it would
/// poison every Mac that has the app, which is every Mac that has Tailscale at
/// all. That is not hypothetical: it shipped, and it is what made **Settings ->
/// Mobile Access** show a Sign in button that silently did nothing on a machine
/// already on its tailnet.
///
/// macOS users get a real CLI from Homebrew or from the app's own
/// `/usr/local/bin/tailscale` symlink. With neither, the actions are
/// unavailable and the UI says so, while state reporting is unaffected because
/// it does not use this at all.
pub const TAILSCALE_CANDIDATES: &[&str] = &[
    "/usr/local/bin/tailscale", // the macOS app's own CLI symlink, or Intel Homebrew
    "/opt/homebrew/bin/tailscale", // Homebrew, Apple Silicon
    "/usr/bin/tailscale",       // Linux distro package
];

/// Pure resolution of the Tailscale CLI: an explicit override, else the first
/// candidate that exists, else the bare name.
///
/// Resolving by ABSOLUTE PATH is the whole point. A bare
/// `Command::new("tailscale")` works in dev (rich shell `PATH`) and fails in
/// every packaged process, which Finder/launchd start with **no `PATH` at all**.
/// Same shape as `runtime::claude_code::resolve_claude_binary`. The bare-name
/// fallback keeps a custom install working wherever a `PATH` does exist.
pub fn resolve_tailscale_binary(
    env_override: Option<&str>,
    candidates: &[&str],
    exists: impl Fn(&str) -> bool,
) -> String {
    if let Some(p) = env_override {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    candidates
        .iter()
        .find(|c| exists(c))
        .map(|c| (*c).to_string())
        .unwrap_or_else(|| "tailscale".to_string())
}

/// [`resolve_tailscale_binary`] against the process env + the real filesystem.
pub fn tailscale_binary() -> String {
    resolve_tailscale_binary(
        std::env::var("LUCIDOS_TAILSCALE_BIN").ok().as_deref(),
        TAILSCALE_CANDIDATES,
        |p| Path::new(p).exists(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tailscale_binary_prefers_an_absolute_path_over_the_bare_name() {
        // The packaged-process case: no PATH, so an absolute path or nothing.
        let only_homebrew = |p: &str| p == "/opt/homebrew/bin/tailscale";
        assert_eq!(
            resolve_tailscale_binary(None, TAILSCALE_CANDIDATES, only_homebrew),
            "/opt/homebrew/bin/tailscale"
        );
        // With several present, the first listed CLI wins.
        assert_eq!(
            resolve_tailscale_binary(None, TAILSCALE_CANDIDATES, |_| true),
            "/usr/local/bin/tailscale"
        );
    }

    #[test]
    fn tailscale_binary_env_override_wins_over_every_candidate() {
        assert_eq!(
            resolve_tailscale_binary(Some("/custom/ts"), TAILSCALE_CANDIDATES, |_| true),
            "/custom/ts"
        );
        // Set-but-empty is not an override; fall through to the candidates.
        assert_eq!(
            resolve_tailscale_binary(Some("  "), TAILSCALE_CANDIDATES, |_| true),
            "/usr/local/bin/tailscale"
        );
    }

    #[test]
    fn tailscale_candidates_exclude_the_macos_gui_executable() {
        // See TAILSCALE_CANDIDATES: the GUI binary exits 0 without answering,
        // and resolution stops at the first candidate that EXISTS, so listing
        // it would shadow the working CLI next to it.
        assert!(
            !TAILSCALE_CANDIDATES
                .iter()
                .any(|c| c.contains("Tailscale.app")),
            "the macOS GUI executable must never be a CLI candidate"
        );
        // No known location present: keep a bare name so a custom install still
        // resolves wherever a PATH exists (dev shells, most Linux).
        assert_eq!(
            resolve_tailscale_binary(None, TAILSCALE_CANDIDATES, |_| false),
            "tailscale"
        );
    }
}
