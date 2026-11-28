use std::path::{Path, PathBuf};

use lucidos_core::core::event_hooks::HOOK_DEFINITION_FILE;
use sha2::{Digest, Sha256};

const DOMAIN: &[u8] = b"lucidos-event-hook-content-v2\0";

pub(crate) const FINDER_METADATA: &str = ".DS_Store";

pub(crate) fn of(dir: &Path, definition: &str) -> Result<String, String> {
    let mut files = Vec::new();
    collect(dir, &PathBuf::new(), "", &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    for (name, path) in files {
        let body = if name == HOOK_DEFINITION_FILE {
            definition.as_bytes().to_vec()
        } else {
            std::fs::read(dir.join(&path)).map_err(|e| format!("'{name}' cannot be read: {e}"))?
        };
        for part in [name.as_bytes(), &body] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part);
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) fn file_names(dir: &Path) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    collect(dir, &PathBuf::new(), "", &mut files)?;
    Ok(files.into_iter().map(|(name, _)| name).collect())
}

fn collect(
    dir: &Path,
    rel: &Path,
    prefix: &str,
    files: &mut Vec<(String, PathBuf)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir.join(rel))
        .map_err(|e| format!("the hook's folder cannot be read: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("the hook's folder cannot be read: {e}"))?;
        let file_name = entry.file_name();
        if file_name == FINDER_METADATA {
            continue;
        }
        let path = rel.join(&file_name);
        let Some(file_name) = file_name.to_str() else {
            return Err(format!("'{}' is not a UTF-8 name.", path.to_string_lossy()));
        };
        let name = match prefix {
            "" => file_name.to_string(),
            prefix => format!("{prefix}/{file_name}"),
        };
        let kind = entry
            .file_type()
            .map_err(|e| format!("'{name}' cannot be read: {e}"))?;
        if kind.is_symlink() {
            return Err(format!(
                "'{name}' is a link. A hook's folder holds plain files only, so its \
                 arming covers everything the hook can run."
            ));
        }
        if kind.is_dir() {
            collect(dir, &path, &name, files)?;
        } else if kind.is_file() {
            files.push((name, path));
        } else {
            return Err(format!("'{name}' is not a plain file or folder."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFINITION: &str =
        "name = \"Gate\"\nevent_type = \"MessageReceived\"\nscript = \"check.py\"\n";

    fn hook(root: &Path, script: &str) -> PathBuf {
        let dir = root.join("gate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(HOOK_DEFINITION_FILE), DEFINITION).unwrap();
        std::fs::write(dir.join("check.py"), script).unwrap();
        dir
    }

    #[test]
    fn the_same_content_hashes_the_same() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let digest = of(&hook(a.path(), "print('allow')\n"), DEFINITION).unwrap();
        assert_eq!(digest.len(), 64, "{digest}");
        assert_eq!(
            digest,
            of(&hook(b.path(), "print('allow')\n"), DEFINITION).unwrap()
        );
    }

    #[test]
    fn an_edit_to_the_script_or_the_definition_changes_the_digest() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook(root.path(), "print('allow')\n");
        let armed = of(&dir, DEFINITION).unwrap();
        let edited = format!("{DEFINITION}timeout_secs = 5\n");
        assert_ne!(armed, of(&dir, &edited).unwrap());
        hook(root.path(), "print('refuse')\n");
        assert_ne!(armed, of(&dir, DEFINITION).unwrap());
    }

    #[test]
    fn the_digest_binds_the_parsed_definition_text() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook(root.path(), "print('allow')\n");
        let armed = of(&dir, DEFINITION).unwrap();
        std::fs::write(dir.join(HOOK_DEFINITION_FILE), "edited").unwrap();
        assert_eq!(armed, of(&dir, DEFINITION).unwrap());
    }

    #[test]
    fn a_change_anywhere_in_the_folder_changes_the_digest() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook(root.path(), "import lib.rules\n");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("lib/rules.py"), "ALLOW = True\n").unwrap();
        let armed = of(&dir, DEFINITION).unwrap();

        std::fs::write(dir.join("lib/rules.py"), "ALLOW = False\n").unwrap();
        let edited = of(&dir, DEFINITION).unwrap();
        assert_ne!(armed, edited);

        std::fs::write(dir.join("lib/extra.py"), "").unwrap();
        assert_ne!(edited, of(&dir, DEFINITION).unwrap());
    }

    #[test]
    fn names_and_bodies_cannot_trade_bytes() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let one = hook(a.path(), "x");
        std::fs::write(one.join("ab"), "c").unwrap();
        let two = hook(b.path(), "x");
        std::fs::write(two.join("a"), "bc").unwrap();
        assert_ne!(of(&one, DEFINITION).unwrap(), of(&two, DEFINITION).unwrap());
    }

    #[test]
    fn finder_metadata_is_no_content() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook(root.path(), "print('allow')\n");
        let armed = of(&dir, DEFINITION).unwrap();
        std::fs::write(dir.join(FINDER_METADATA), "view state").unwrap();
        assert_eq!(armed, of(&dir, DEFINITION).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn a_backslash_name_is_its_own_file() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook(root.path(), "print('allow')\n");
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(dir.join("lib/rules.py"), "ALLOW = True\n").unwrap();
        std::fs::write(dir.join("lib\\rules.py"), "ALLOW = True\n").unwrap();
        let armed = of(&dir, DEFINITION).unwrap();

        std::fs::write(dir.join("lib\\rules.py"), "ALLOW = False\n").unwrap();
        assert_ne!(armed, of(&dir, DEFINITION).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_anywhere_in_the_folder_has_no_digest() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook(root.path(), "print('allow')\n");
        std::fs::write(root.path().join("outside.py"), "").unwrap();
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::os::unix::fs::symlink(root.path().join("outside.py"), dir.join("lib/x.py")).unwrap();
        let err = of(&dir, DEFINITION).unwrap_err();
        assert!(err.contains("lib/x.py"), "{err}");
    }
}
