use std::path::Path;

pub fn exclude(dir: &Path, what: &str) {
    match lucidos_file_backup_exclusion::ensure_excluded(dir) {
        Ok(outcome) if outcome.changed() => crate::log_legacy!(
            "[Gateway] excluded the {} from file-level backup: {}",
            what,
            dir.display()
        ),
        Ok(_) => {}
        Err(e) => crate::log_legacy!(
            "[Gateway] could not exclude the {} at {} from file-level backup: {}",
            what,
            dir.display(),
            e
        ),
    }
}
