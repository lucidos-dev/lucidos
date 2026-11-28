use std::path::Path;

pub fn wanted(sandbox_active: bool, macos: bool) -> bool {
    sandbox_active && macos
}

pub fn script(cli: &Path) -> String {
    let quoted = cli.display().to_string().replace('\'', r"'\''");
    format!("#!/bin/sh\nexec '{quoted}' ps \"$@\"\n")
}

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

    #[cfg(unix)]
    #[test]
    fn a_racing_reader_never_sees_a_half_written_shim() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicBool, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let old = format!("#!/bin/sh\n{}", "# old\n".repeat(200_000));
        let new = format!("#!/bin/sh\n{}", "# new\n".repeat(200_000));
        write(dir.path(), &old).unwrap();
        let done = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let shim = dir.path().join("ps");
                let mut reads = 0;
                while !done.load(Ordering::Relaxed) || reads < 50 {
                    let text = std::fs::read_to_string(&shim).unwrap();
                    assert!(text == old || text == new, "saw a partial shim");
                    let mode = std::fs::metadata(&shim).unwrap().permissions().mode();
                    assert_eq!(mode & 0o111, 0o111, "shim not executable");
                    reads += 1;
                }
            });
            write(dir.path(), &new).unwrap();
            done.store(true, Ordering::Relaxed);
            reader.join().unwrap();
        });
    }

    #[test]
    fn the_ps_shim_execs_the_cli_and_quotes_its_path() {
        assert_eq!(
            script(Path::new("/opt/it's here/lucidos")),
            "#!/bin/sh\nexec '/opt/it'\\''s here/lucidos' ps \"$@\"\n"
        );
    }
}
