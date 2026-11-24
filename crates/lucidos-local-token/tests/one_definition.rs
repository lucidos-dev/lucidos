//! The loopback client setup and the certificate's file name each have one
//! definition (invariant I17 of `docs/plans/2026-11-19-desktop-window-https.md`).
//! A second copy drifts on the next change, so this scans every Rust source in
//! the tree for one.

use std::path::{Path, PathBuf};

/// Each guarded token, and the only files allowed to hold it.
const GUARDED: &[(&str, &[&str])] = &[
    (
        "danger_accept_invalid_certs",
        &[
            "crates/lucidos-local-token/src/client.rs",
            // The opt-in transport for a provider's own self-signed backend.
            // It never reaches a loopback listener of ours.
            "crates/lucidos-agents-api/src/api/proxy.rs",
            // A test naming it as forbidden on the tailnet probe.
            "crates/lucidos-agents-api/src/api/settings.rs",
        ],
    ),
    (
        "\"loopback-ca.pem\"",
        &["crates/lucidos-local-token/src/loopback_cert.rs"],
    ),
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("the crate sits two levels under the repository root")
        .to_path_buf()
}

fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read a source directory") {
        let path = entry.expect("read a directory entry").path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if path.is_dir() {
            if !matches!(name.as_ref(), "target" | "vendor" | "node_modules") {
                rust_sources(&path, found);
            }
        } else if name.ends_with(".rs") {
            found.push(path);
        }
    }
}

#[test]
fn each_guarded_token_lives_only_where_it_is_defined() {
    let root = repo_root();
    let mut sources = Vec::new();
    rust_sources(&root.join("crates"), &mut sources);
    assert!(
        sources.len() > 100,
        "the scan found too few sources to mean anything"
    );
    let mut strays = Vec::new();
    for path in &sources {
        let relative = path.strip_prefix(&root).unwrap();
        if relative == Path::new(file!()) {
            continue;
        }
        let text = std::fs::read_to_string(path).unwrap_or_default();
        for (token, allowed) in GUARDED {
            let permitted = allowed.iter().any(|a| relative == Path::new(a));
            if !permitted && text.contains(token) {
                strays.push(format!("{} holds {token}", relative.display()));
            }
        }
    }
    assert!(
        strays.is_empty(),
        "build loopback clients through `lucidos_local_token::client`, and name the \
         certificate file through `loopback_cert::PUBLIC_CERT_FILE`:\n{}",
        strays.join("\n")
    );
}
