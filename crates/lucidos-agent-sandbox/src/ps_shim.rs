//! The `ps` stand-in a sandboxed agent runs on macOS.
use std::path::Path;

/// macOS runs `/bin/ps` setuid, which its sandbox refuses. A sandboxed agent
/// gets the `lucidos ps` stand-in in its place, and nobody else does.
pub fn wanted(sandbox_active: bool, macos: bool) -> bool {
    sandbox_active && macos
}

/// The shim script that runs `ps` through the CLI at `cli`.
pub fn script(cli: &Path) -> String {
    let quoted = cli.display().to_string().replace('\'', r"'\''");
    format!("#!/bin/sh\nexec '{quoted}' ps \"$@\"\n")
}

/// Write `script` as `dir/ps`, executable, through a rename so a spawn racing
/// this one never runs a half-written file.
pub fn write(dir: &Path, script: &str) -> std::io::Result<()> {
    let shim = dir.join("ps");
    if std::fs::read_to_string(&shim).is_ok_and(|current| current == script) {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    let temp = dir.join(format!("ps.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temp, script)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&temp, &shim)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_active_sandbox_on_macos_gets_the_ps_shim() {
        for (active, macos, expected) in [
            (true, true, true),
            (true, false, false),
            (false, true, false),
            (false, false, false),
        ] {
            assert_eq!(wanted(active, macos), expected, "{active} {macos}");
        }
    }

    #[test]
    fn the_ps_shim_execs_the_cli_and_quotes_its_path() {
        assert_eq!(
            script(Path::new("/opt/it's here/lucidos")),
            "#!/bin/sh\nexec '/opt/it'\\''s here/lucidos' ps \"$@\"\n"
        );
    }
}
