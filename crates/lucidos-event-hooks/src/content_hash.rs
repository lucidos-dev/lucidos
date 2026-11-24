//! The content an arming binds to (ADR 0746): a hook's definition file and the
//! script it names. Any edit to either changes the digest, so an armed hook
//! that was edited no longer runs as armed.

use std::path::Path;

use sha2::{Digest, Sha256};

use crate::contained;

/// Names the encoding, so a later change to it cannot match an old digest.
const DOMAIN: &[u8] = b"lucidos-event-hook-content-v1\0";

/// The hex SHA-256 of the hook in `dir`, whose definition file reads
/// `definition` and names `script`.
pub(crate) fn of(dir: &Path, definition: &str, script: &str) -> Result<String, String> {
    let body = std::fs::read(contained(dir, script)?)
        .map_err(|e| format!("script: '{script}' cannot be read: {e}"))?;
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    // Each part carries its length, so no byte can move from one to the next.
    for part in [definition.as_bytes(), script.as_bytes(), &body] {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFINITION: &str =
        "name = \"Gate\"\nevent_type = \"MessageReceived\"\nscript = \"check.py\"\n";

    fn hook(root: &Path, script: &str) -> std::path::PathBuf {
        let dir = root.join("gate");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("check.py"), script).unwrap();
        dir
    }

    #[test]
    fn the_same_content_hashes_the_same() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let digest = of(&hook(a.path(), "print('allow')\n"), DEFINITION, "check.py").unwrap();
        assert_eq!(digest.len(), 64, "{digest}");
        assert_eq!(
            digest,
            of(&hook(b.path(), "print('allow')\n"), DEFINITION, "check.py").unwrap()
        );
    }

    #[test]
    fn an_edit_to_the_script_or_the_definition_changes_the_digest() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook(root.path(), "print('allow')\n");
        let armed = of(&dir, DEFINITION, "check.py").unwrap();
        let edited = format!("{DEFINITION}timeout_secs = 5\n");
        assert_ne!(armed, of(&dir, &edited, "check.py").unwrap());
        hook(root.path(), "print('refuse')\n");
        assert_ne!(armed, of(&dir, DEFINITION, "check.py").unwrap());
    }

    #[test]
    fn a_missing_script_or_one_outside_the_folder_has_no_digest() {
        let root = tempfile::tempdir().unwrap();
        let dir = hook(root.path(), "print('allow')\n");
        std::fs::write(root.path().join("outside.py"), "").unwrap();
        assert!(of(&dir, DEFINITION, "../outside.py").is_err());
        std::fs::remove_file(dir.join("check.py")).unwrap();
        let err = of(&dir, DEFINITION, "check.py").unwrap_err();
        assert!(err.contains("check.py"), "{err}");
    }
}
