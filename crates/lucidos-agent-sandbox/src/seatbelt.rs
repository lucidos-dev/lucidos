use std::path::{Path, PathBuf};

use crate::Policy;

pub const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";

pub fn profile(policy: &Policy) -> String {
    let mut out = String::from("(version 1)\n(allow default)\n");
    rules(&mut out, "deny", &policy.hidden, true);
    rules(&mut out, "allow", &policy.open_within_hidden, true);
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
    rules(&mut out, "deny", &policy.private, true);
    if let Some(roots) = &policy.write_scope {
        out.push_str("(deny file-write* (require-not (require-any");
        for root in roots {
            out.push_str(&format!(" (subpath {})", quoted(root)));
        }
        out.push_str(")))\n");
    }
    for socket in &policy.connectable_sockets {
        out.push_str(&format!(
            "(allow network-outbound (remote unix-socket (path-literal {})))\n",
            quoted(socket)
        ));
    }
    for ancestor in fixed_ancestors(policy) {
        out.push_str(&format!(
            "(deny file-write-unlink (literal {}))\n",
            quoted(&ancestor)
        ));
    }
    out
}

fn fixed_ancestors(policy: &Policy) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for root in policy
        .hidden
        .iter()
        .chain(&policy.read_only)
        .chain(&policy.private)
    {
        for ancestor in root.ancestors().skip(1) {
            if ancestor.parent().is_some() && !out.iter().any(|p| p == ancestor) {
                out.push(ancestor.to_path_buf());
            }
        }
    }
    out
}

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

fn quoted(path: &Path) -> String {
    let text = path.to_string_lossy();
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

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
