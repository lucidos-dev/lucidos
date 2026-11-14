//! The diagnostic log: this engine's own `log!` lines, sanitized, in a
//! rotated JSONL file under `<ws>/.lucidos/diagnostic-log/` (ADR 0667).
//!
//! It never reads the host log, a service log, journald or another workspace.
//! Every path here derives from the workspace root alone.

pub mod query;
pub mod sanitize;
pub mod writer;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use sanitize::register_secret_values;

static SINK: OnceLock<writer::Sink> = OnceLock::new();

/// Where a workspace keeps its diagnostic log.
pub fn dir_for(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join("diagnostic-log")
}

/// Start the writer thread for `workspace`. Call once, after the stored
/// environment variables reach the process env, so their secret values get
/// masked. A later call changes nothing.
pub fn start(workspace: &Path) -> std::io::Result<()> {
    if SINK.get().is_some() {
        return Ok(());
    }
    let env: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    register_secret_values(crate::core::subprocess_secret_values(
        &env,
        crate::core::inherited_secret_env(),
    ));
    let segments = writer::Segments::open(&dir_for(workspace))?;
    let (sink, rx, dropped) = writer::channel(writer::CHANNEL_CAPACITY);
    std::thread::Builder::new()
        .name("diagnostic-log".into())
        .spawn(move || writer::drain(rx, segments, &dropped))?;
    // A racing second start loses here, and its writer ends with its sink.
    let _ = SINK.set(sink);
    Ok(())
}

/// Whether [`start`] succeeded in this process.
pub fn started() -> bool {
    SINK.get().is_some()
}

/// Hand one `log!` line to the writer. Returns at once, and does nothing
/// when the sink never started (tests, other binaries).
pub fn record(label: &'static str, msg: String) {
    if let Some(sink) = SINK.get() {
        sink.record(label, msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_lives_under_the_workspace_root_only() {
        let workspace = Path::new("/home/user/workspaces/myws");
        assert_eq!(
            dir_for(workspace),
            Path::new("/home/user/workspaces/myws/.lucidos/diagnostic-log")
        );
    }
}
