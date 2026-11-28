pub use lucidos_diagnostic_log::{query, sanitize, writer};

use std::path::{Path, PathBuf};

pub use sanitize::register_secret_values;

pub fn dir_for(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join("diagnostic-log")
}

/// INVARIANT: Start the writer thread for `workspace`. Call once, after the stored
/// environment variables reach the process env, so their secret values get
/// masked. A later call changes nothing.
pub fn start(workspace: &Path) -> std::io::Result<()> {
    if started() {
        return Ok(());
    }
    let env: Vec<(String, String)> = std::env::vars_os()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    register_secret_values(crate::core::subprocess_secret_values(
        &env,
        crate::core::inherited_secret_env(),
    ));
    lucidos_diagnostic_log::start(&dir_for(workspace))?;
    lucidos_diagnostic_log::install_panic_hook();
    lucidos_diagnostic_log::stderr::install()?;
    Ok(())
}

pub fn started() -> bool {
    lucidos_diagnostic_log::started()
}

pub fn record(label: &'static str, msg: String) {
    lucidos_diagnostic_log::record_legacy(label, msg);
}

/// INVARIANT: Max entries one batched `/internal/client-logs` request may carry, so a
/// misbehaving client can't flood the engine.log tail in a single POST. The
/// frontend perf queue flushes well under this.
pub const CLIENT_LOG_MAX_BATCH: usize = 100;

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
