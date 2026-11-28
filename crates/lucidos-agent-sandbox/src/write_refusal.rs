use std::path::Path;

pub const REFUSED_WRITE: &str = "Sandbox refused a write to ";

pub fn refused_path(output: &str) -> Option<&Path> {
    const REFUSALS: [&str; 3] = [
        "Permission denied",
        "Operation not permitted",
        "Read-only file system",
    ];
    const VERBS: [&str; 3] = [
        "cannot create directory ",
        "cannot create ",
        "cannot touch ",
    ];
    output.lines().rev().find_map(|line| {
        let path = if let Some(path) = line.strip_prefix(REFUSED_WRITE) {
            path
        } else if line.starts_with("PermissionError:") || line.starts_with("OSError:") {
            REFUSALS.iter().find_map(|refusal| {
                let (_, path) = line.split_once(&format!("{refusal}: "))?;
                let quote = path.chars().next().filter(|c| matches!(c, '\'' | '"'))?;
                let path = path.strip_prefix(quote)?.strip_suffix(quote)?;
                (!path.contains(['\\', quote])).then_some(path)
            })?
        } else {
            let location = REFUSALS
                .iter()
                .find_map(|refusal| line.strip_suffix(&format!(": {refusal}")))?;
            let location = location
                .split_once(": ")
                .map_or(location, |(prefix, rest)| {
                    let has_line_number = rest.split_once(": ").is_some_and(|(line, _)| {
                        line.strip_prefix("line ")
                            .unwrap_or(line)
                            .parse::<u32>()
                            .is_ok()
                    });
                    if prefix.starts_with('/') && !has_line_number {
                        location
                    } else if has_line_number {
                        rest.split_once(": ").map_or(rest, |(_, path)| path)
                    } else {
                        rest
                    }
                });
            let path = VERBS
                .iter()
                .find_map(|verb| location.strip_prefix(verb))
                .unwrap_or(location);
            path.strip_prefix('\'')
                .and_then(|p| p.strip_suffix('\''))
                .or_else(|| path.strip_prefix('"').and_then(|p| p.strip_suffix('"')))
                .unwrap_or(path)
        };
        let path = Path::new(path);
        path.is_absolute().then_some(path)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_diagnostics_require_a_path() {
        assert_eq!(
            refused_path("sh: /outside/a b: Permission denied"),
            Some(Path::new("/outside/a b"))
        );
        assert_eq!(
            refused_path("PermissionError: [Errno 1] Operation not permitted: '/outside/file'"),
            Some(Path::new("/outside/file"))
        );
        assert_eq!(
            refused_path("sh: 1: cannot create /outside/a: Read-only file system"),
            Some(Path::new("/outside/a"))
        );
        assert_eq!(
            refused_path("OSError: [Errno 30] Read-only file system: '/outside/file'"),
            Some(Path::new("/outside/file"))
        );
        assert_eq!(
            refused_path("touch: cannot touch '/outside/b': Read-only file system"),
            Some(Path::new("/outside/b"))
        );
        assert_eq!(
            refused_path("mkdir: cannot create directory '/outside/c': Permission denied"),
            Some(Path::new("/outside/c"))
        );
        assert_eq!(
            refused_path("OSError: [Errno 28] No space left on device: '/outside/file'"),
            None
        );
        assert_eq!(refused_path("Permission denied"), None);
        assert_eq!(refused_path("sh: ../outside/file: Permission denied"), None);
        assert_eq!(
            refused_path("/workspace/script.sh: 1: cannot create /outside/a: b: Permission denied"),
            Some(Path::new("/outside/a: b"))
        );
        assert_eq!(
            refused_path(
                "PermissionError: [Errno 13] Permission denied: '/outside/a' -> '/elsewhere/b'"
            ),
            None
        );
        assert_eq!(
            refused_path("open: /outside/file: No such file or directory"),
            None
        );
    }
}
