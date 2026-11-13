//! The macOS backend: a seatbelt profile run through `sandbox-exec`.
//!
//! The profile allows by default and denies what the [`Policy`] names. When
//! two rules match one operation, the later one wins. So each exception
//! follows the denial it carves into.
//!
//! A seatbelt cannot nest under one that denies anything: the inner
//! `sandbox_apply` fails with EPERM (Phase 0 of
//! `docs/plans/2026-11-10-os-sandbox-for-agent-subprocesses.md`). Codex and
//! Claude Code therefore run with their own sandboxes off inside ours.

use std::path::{Path, PathBuf};

use crate::Policy;

/// The seatbelt launcher. It ships with every macOS release Lucidos supports.
pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

/// The profile text for `policy`, as `sandbox-exec -p` takes it.
pub fn profile(policy: &Policy) -> String {
    let mut out = String::from("(version 1)\n(allow default)\n");
    rules(&mut out, "deny", &policy.hidden, true);
    rules(&mut out, "allow", &policy.open_within_hidden, true);
    // Path resolution needs metadata on the exact hidden folders above each reopening.
    let mut ancestors = Vec::new();
    for open in &policy.open_within_hidden {
        for ancestor in open.ancestors().skip(1) {
            if policy.hidden.iter().any(|root| ancestor.starts_with(root))
                && !ancestors.contains(&ancestor)
            {
                ancestors.push(ancestor);
                out.push_str(&format!(
                    "(allow file-read-metadata (literal {}))\n",
                    quoted(ancestor)
                ));
            }
        }
    }
    rules(&mut out, "deny", &policy.read_only, false);
    rules(&mut out, "allow", &policy.writable_within_read_only, false);
    for ancestor in fixed_ancestors(policy) {
        out.push_str(&format!(
            "(deny file-write-unlink (literal {}))\n",
            quoted(&ancestor)
        ));
    }
    out
}

/// Every folder above a hidden or read-only root, below `/`. A seatbelt rule
/// binds a path, not an inode, so renaming one of these folders would carry
/// the root out from under its rule. Landlock binds inodes and needs none.
fn fixed_ancestors(policy: &Policy) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in policy.hidden.iter().chain(&policy.read_only) {
        for ancestor in root.ancestors().skip(1) {
            if ancestor.parent().is_some() && !out.iter().any(|p| p == ancestor) {
                out.push(ancestor.to_path_buf());
            }
        }
    }
    out
}

/// One rule per path. `whole` covers reads and unix-socket connects too, not
/// only writes.
fn rules(out: &mut String, verdict: &str, paths: &[PathBuf], whole: bool) {
    for path in paths {
        let subpath = format!("(subpath {})", quoted(path));
        if whole {
            out.push_str(&format!("({verdict} file-read* file-write* {subpath})\n"));
            out.push_str(&format!(
                "({verdict} network-outbound (remote unix-socket {subpath}))\n"
            ));
        } else {
            out.push_str(&format!("({verdict} file-write* {subpath})\n"));
        }
    }
}

/// `path` as an SBPL string literal.
fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Whether `sandbox-exec` can apply a profile that denies something here.
///
/// A trial, not a check for the file: inside another seatbelt, as when the
/// engine itself runs sandboxed, the binary exists and every apply fails.
#[cfg(target_os = "macos")]
pub fn probe() -> crate::Availability {
    use crate::Availability;
    let trial = format!(
        "(version 1)\n(allow default)\n(deny file-read* (subpath {}))\n",
        quoted(Path::new("/var/empty/lucidos-sandbox-probe"))
    );
    match std::process::Command::new(SANDBOX_EXEC)
        .arg("-p")
        .arg(trial)
        .arg("/usr/bin/true")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
    {
        Ok(out) if out.status.success() => Availability::Available,
        Ok(out) => Availability::Unavailable(format!(
            "{SANDBOX_EXEC} could not apply a profile ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )),
        Err(e) => Availability::Unavailable(format!("{SANDBOX_EXEC} could not run: {e}")),
    }
}

#[cfg(test)]
#[path = "seatbelt_tests.rs"]
mod tests;
