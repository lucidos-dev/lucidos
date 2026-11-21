//! This gateway's sanitized diagnostic sink. It never reads a host log or links
//! the engine; the caller supplies the gateway's resolved private data dir.
use lucidos_diagnostic_log::{self as shared, Destination, Line};
use std::path::{Path, PathBuf};

pub fn dir_for(app_data: &Path) -> PathBuf {
    app_data.join("diagnostic-log")
}

pub fn start(app_data: &Path) -> std::io::Result<()> {
    if shared::started() {
        return Ok(());
    }
    shared::sanitize::register_secret_values(std::env::vars_os().filter_map(|(name, value)| {
        let name = name.into_string().ok()?;
        let value = value.into_string().ok()?;
        secret_env_name(&name).then_some(value)
    }));
    shared::start(&dir_for(app_data))?;
    shared::install_panic_hook();
    shared::stderr::install()?;
    Ok(())
}

// The gateway owns this policy independently of the engine's subprocess env.
// Paths are configuration, while opaque credential values must be registered.
fn secret_env_name(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    if matches!(
        name.as_str(),
        "GOOGLE_APPLICATION_CREDENTIALS"
            | "LUCIDOS_TLS_KEY"
            | "LUCIDOS_PREVIEW_TLS_KEY"
            | "LUCIDOS_PREVIEW_USER_TLS_KEY"
    ) {
        return false;
    }
    name == "PGPASSWORD"
        || name == "DATABASE_URL"
        || name == "LUCIDOS_DATABASE_URL"
        || name.starts_with("CRED_")
        || name.starts_with("OAUTH_")
        || [
            "_TOKEN",
            "_SECRET",
            "_PASSWORD",
            "_PASSWD",
            "_KEY",
            "_PAT",
            "_CREDENTIALS",
        ]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

/// Register before returning a freshly minted or loaded credential to a caller.
pub fn secret(value: String) -> String {
    shared::sanitize::register_secret_values([value.clone()]);
    value
}

pub fn typed(line: Line) {
    output::host(&line.render(Destination::Owner));
    shared::record("Gateway", line);
}

pub fn legacy(message: String) {
    output::host(&message);
    shared::record_legacy("Gateway", message);
}

pub fn boot_failure(message: &str) {
    // Keep the exact marker the desktop shell parses, including raw host text.
    shared::record(
        "Gateway",
        shared::line!("boot failed: {}", shared::private(message)),
    );
    shared::flush(std::time::Duration::from_secs(1));
    // The process may exit immediately: write to the saved host descriptor
    // synchronously rather than queue the shell's boot marker through the tee.
    shared::stderr::host_error(message);
}

// Metadata probes are user-facing machine-readable stdout, deliberately outside
// diagnostics. They return before the sink starts or any data directory opens.
pub fn probe(value: std::fmt::Arguments<'_>) {
    output::probe(value);
}

// Test skip notices retain their original stderr text, outside the sink.
#[cfg(test)]
pub fn test_notice(value: std::fmt::Arguments<'_>) {
    shared::stderr::host_error(&value.to_string());
}

#[allow(clippy::print_stdout)]
mod output {
    pub fn host(message: &str) {
        println!(
            "{} [pid:{}] {message}",
            chrono::Local::now().format(lucidos_gateway_constants::LOG_STAMP_FORMAT),
            std::process::id()
        );
    }

    pub fn probe(value: std::fmt::Arguments<'_>) {
        println!("{value}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_credentials_are_distinct_from_paths() {
        for name in [
            "PGPASSWORD",
            "DATABASE_URL",
            "GH_TOKEN",
            "GATEWAY_TEST_SECRET",
            "CRED_EXAMPLE",
            "OAUTH_EXAMPLE",
            "LUCIDOS_DATABASE_URL",
        ] {
            assert!(secret_env_name(name), "{name}");
        }
        for name in [
            "HOME",
            "PATH",
            "LUCIDOS_TLS_KEY",
            "LUCIDOS_PREVIEW_TLS_KEY",
            "LUCIDOS_PREVIEW_USER_TLS_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
        ] {
            assert!(!secret_env_name(name), "{name}");
        }
    }

    #[test]
    fn startup_claims_the_gateway_before_opening_its_sink() {
        let source = include_str!("server.rs");
        let run = &source[source.find("pub async fn run(").unwrap()..];
        let claim = run.find("claim_data_dir(&app_data").unwrap();
        let start = run
            .find("crate::diagnostic_log::start(&app_data)?;")
            .unwrap();
        let registry = run.find("let registry_path").unwrap();
        assert!(claim < start && start < registry);
    }

    #[test]
    fn log_is_scoped_to_the_resolved_gateway_data_dir() {
        assert_eq!(
            dir_for(Path::new("/srv/gateway")),
            Path::new("/srv/gateway/diagnostic-log")
        );
    }
}
