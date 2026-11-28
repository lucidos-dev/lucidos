use crate::BoxError;
use lucidos_home::disposable::Scope;

pub fn from_args(args: Vec<std::ffi::OsString>) -> Result<Option<Scope>, BoxError> {
    if !args.iter().any(|arg| arg == "--disposable") {
        return Ok(None);
    }
    if args.len() != 2 || args[0] != "--disposable" {
        return Err("expected --disposable <fresh-scope>".into());
    }
    let scope = Scope::open(std::path::Path::new(&args[1]))?;
    for (key, expected) in [
        ("HOME", scope.home()),
        ("LUCIDOS_PRIVATE_DIR", scope.private_dir()),
        ("LUCIDOS_GATEWAY_DATA", scope.app_data()),
    ] {
        if std::env::var_os(key).as_deref() != Some(expected.as_os_str()) {
            return Err(format!("{key} does not match disposable scope").into());
        }
    }
    for (key, _) in std::env::vars_os() {
        let key = key.to_string_lossy();
        if key.starts_with("LUCIDOS_")
            && !matches!(
                key.as_ref(),
                "LUCIDOS_PRIVATE_DIR"
                    | "LUCIDOS_AGENT_SANDBOXED"
                    | "LUCIDOS_SECRET_KEY_STORE"
                    | "LUCIDOS_GATEWAY_DATA"
                    | "LUCIDOS_API_PORT"
                    | "LUCIDOS_GATEWAY_PG_BACKEND"
                    | "LUCIDOS_PG_BIN_DIR"
                    | "LUCIDOS_PG_LIB_DIR"
                    | "LUCIDOS_ENGINE_BIN"
                    | "LUCIDOS_CLI_BIN"
                    | "LUCIDOS_STATIC_DIR"
                    | "LUCIDOS_SDK_DIR"
                    | "LUCIDOS_SYSTEM_KNOWHOW_DIR"
                    | "LUCIDOS_SYSTEM_WIDGETS_DIR"
                    | "LUCIDOS_BOOT_WITHOUT_PROVIDER"
                    | "LUCIDOS_PACKAGED"
                    | "LUCIDOS_GATEWAY_BIND_ADDR"
                    | "LUCIDOS_HOOK_PORT"
                    | "LUCIDOS_PUBLIC_PORT"
            )
        {
            return Err(format!("{key} is forbidden in disposable mode").into());
        }
    }
    for key in [
        "DATABASE_URL",
        "PGHOST",
        "PGPORT",
        "PGUSER",
        "PGPASSWORD",
        "PGDATABASE",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "CODEX_HOME",
        "CLAUDE_CONFIG_DIR",
    ] {
        if std::env::var_os(key).is_some() {
            return Err(format!("{key} is forbidden in disposable mode").into());
        }
    }
    if std::env::var_os("FASTEMBED_CACHE_DIR").as_deref()
        != Some(scope.app_data().join("fastembed").as_os_str())
    {
        return Err("disposable cache must be inside its scope".into());
    }
    for (key, value) in [
        ("LUCIDOS_GATEWAY_PG_BACKEND", "embedded"),
        ("LUCIDOS_PACKAGED", "1"),
        ("LUCIDOS_SECRET_KEY_STORE", "file"),
        ("LUCIDOS_GATEWAY_BIND_ADDR", "127.0.0.1"),
        ("LUCIDOS_HOOK_PORT", "0"),
        ("LUCIDOS_PUBLIC_PORT", "0"),
    ] {
        if std::env::var(key).as_deref() != Ok(value) {
            return Err(format!("disposable mode requires {key}={value}").into());
        }
    }
    if std::env::var_os("LUCIDOS_AGENT_SANDBOXED").is_some_and(|value| value != "1") {
        return Err("invalid inherited sandbox marker".into());
    }
    if unsafe { libc::getpgrp() } != std::process::id() as i32 {
        return Err("disposable gateway must own its process group".into());
    }
    if scope.supervisor_pid()? != unsafe { libc::getppid() } as u32 {
        return Err("disposable gateway has no owning supervisor".into());
    }
    scope.claim_gateway()?;
    Ok(Some(scope))
}

pub fn request_allowed(method: &str, path: &str) -> bool {
    match path {
        "/~/api/v1/control/workspaces" => matches!(method, "GET" | "POST"),
        "/~/api/v1/health" | "/~/" | "/~/api/v1/auth/users" => method == "GET",
        _ if path.starts_with("/~/api/v1/control/workspaces/") => {
            let tail = path.trim_start_matches("/~/api/v1/control/workspaces/");
            method == "POST"
                && tail.split_once('/').is_some_and(|(id, callback)| {
                    !id.is_empty()
                        && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
                        && matches!(callback, "boot-phase" | "boot-failure")
                })
        }
        _ => !path.starts_with("/~/") && !path.contains('@') && !path.contains('%'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_creation_listing_and_boot_dependencies_are_exposed_on_control_plane() {
        assert!(request_allowed("POST", "/~/api/v1/control/workspaces"));
        assert!(request_allowed("GET", "/~/api/v1/auth/users"));
        assert!(request_allowed(
            "POST",
            "/~/api/v1/control/workspaces/smoke/boot-phase"
        ));
        assert!(request_allowed(
            "POST",
            "/~/api/v1/control/workspaces/smoke/boot-failure"
        ));
        assert!(!request_allowed("POST", "/~/api/v1/auth/users"));
        assert!(!request_allowed(
            "POST",
            "/~/api/v1/control/workspaces/smoke/restart"
        ));
        for path in [
            "/~/api/v1/control/workspaces/adopt",
            "/~/api/v1/control/gateway/reload",
            "/~/api/v1/remote-gateways",
            "/~/api/v1/auth/pair",
            "/~/api/v1/control/workspaces/restore",
            "/~/api/v1/control/restore-uploads",
        ] {
            assert!(!request_allowed("POST", path), "{path}");
        }
        assert!(!request_allowed(
            "PATCH",
            "/~/api/v1/control/restore-uploads/00000000000000000000000000000000"
        ));
        assert!(!request_allowed("GET", "/remote@example/api/v1/health"));
        assert!(request_allowed("POST", "/smoke/api/v1/triggers"));
    }
    #[test]
    fn partial_or_mixed_launch_contract_is_rejected() {
        assert!(from_args(vec!["--disposable".into()]).is_err());
        assert!(from_args(vec![
            "--disposable".into(),
            "/tmp/run".into(),
            "--restart-after".into()
        ])
        .is_err());
        assert!(from_args(vec![]).unwrap().is_none());
    }

    #[test]
    #[ignore = "subprocess fixture for launch contract tests"]
    fn launch_contract_fixture() {
        let args = vec![
            "--disposable".into(),
            std::env::var_os("TEST_SCOPE").unwrap(),
        ];
        let result = from_args(args);
        if std::env::var_os("TEST_EXPECT_ERROR").is_some() {
            assert!(result.is_err());
        } else {
            assert!(result.unwrap().is_some());
        }
    }

    #[test]
    fn launch_contract_rejects_inherited_overrides_and_persistent_storage() {
        use std::os::unix::process::CommandExt;
        for variant in [
            "valid",
            "without-sandbox-marker",
            "invalid-sandbox-marker",
            "opt-out",
            "tls",
            "shared-pg",
            "external-home",
            "keychain",
            "missing-key-store",
            "registry",
        ] {
            let scratch = tempfile::tempdir().unwrap();
            let scope = Scope::create(&scratch.path().join("run")).unwrap();
            scope.claim_supervisor().unwrap();
            let mut child = scope.command(&std::env::current_exe().unwrap());
            child
                .args([
                    "--ignored",
                    "--exact",
                    "disposable::tests::launch_contract_fixture",
                ])
                .process_group(0)
                .env("TEST_SCOPE", scope.root())
                .env("FASTEMBED_CACHE_DIR", scope.app_data().join("fastembed"))
                .env("LUCIDOS_GATEWAY_PG_BACKEND", "embedded")
                .env("LUCIDOS_PACKAGED", "1")
                .env("LUCIDOS_GATEWAY_BIND_ADDR", "127.0.0.1")
                .env("LUCIDOS_HOOK_PORT", "0")
                .env("LUCIDOS_PUBLIC_PORT", "0");
            match variant {
                "without-sandbox-marker" => {
                    child.env_remove("LUCIDOS_AGENT_SANDBOXED");
                }
                "invalid-sandbox-marker" => {
                    child.env("LUCIDOS_AGENT_SANDBOXED", "0");
                }
                "opt-out" => {
                    child.env("LUCIDOS_ALLOW_WORKTREE_STACK", "1");
                }
                "tls" => {
                    child.env("LUCIDOS_TLS_CERT", "/tmp/external-cert");
                }
                "shared-pg" => {
                    child.env("LUCIDOS_GATEWAY_PG_BACKEND", "docker");
                }
                "external-home" => {
                    child.env("HOME", scratch.path());
                }
                "keychain" => {
                    child.env("LUCIDOS_SECRET_KEY_STORE", "keychain");
                }
                "missing-key-store" => {
                    child.env_remove("LUCIDOS_SECRET_KEY_STORE");
                }
                "registry" => {
                    std::fs::write(scope.app_data().join("workspaces.json"), "{}").unwrap();
                }
                _ => {}
            }
            if !matches!(variant, "valid" | "without-sandbox-marker") {
                child.env("TEST_EXPECT_ERROR", "1");
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success(),
                "{variant}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
        }
    }
}
