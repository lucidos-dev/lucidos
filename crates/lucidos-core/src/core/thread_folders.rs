use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::core::prefs;

pub const MAX_ID_CHARS: usize = 40;
pub const MAX_NAME_CHARS: usize = 40;
pub const MAX_FOLDERS: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThreadFolder {
    pub id: String,
    pub name: String,
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

pub fn validate_document(raw: &str) -> Result<Vec<ThreadFolder>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let key = prefs::THREAD_FOLDERS.key();
    let folders: Vec<ThreadFolder> = serde_json::from_str(trimmed)
        .map_err(|e| format!("'{key}' must be a JSON array of {{id, name}} objects: {e}"))?;
    if folders.len() > MAX_FOLDERS {
        return Err(format!(
            "'{key}' holds at most {MAX_FOLDERS} folders (got {})",
            folders.len()
        ));
    }
    let mut seen = std::collections::HashSet::new();
    for folder in &folders {
        if !is_valid_id(&folder.id) {
            return Err(format!(
                "folder id '{}' must be lowercase letters, digits and hyphens, at most {MAX_ID_CHARS} characters",
                folder.id
            ));
        }
        if !seen.insert(folder.id.as_str()) {
            return Err(format!("folder id '{}' appears twice", folder.id));
        }
        let name = folder.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
            return Err(format!(
                "folder '{}' needs a name of 1 to {MAX_NAME_CHARS} characters",
                folder.id
            ));
        }
    }
    Ok(folders)
}

pub async fn list(pool: &PgPool) -> Vec<ThreadFolder> {
    let Some(raw) = prefs::THREAD_FOLDERS.stored(pool).await else {
        return Vec::new();
    };
    validate_document(&raw).unwrap_or_else(|e| {
        log_legacy!(
            "[ThreadFolders] '{}' is unusable ({e})",
            prefs::THREAD_FOLDERS.key()
        );
        Vec::new()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_value_is_no_folders() {
        assert_eq!(validate_document("  "), Ok(vec![]));
    }

    #[test]
    fn a_well_formed_list_keeps_its_order() {
        let folders =
            validate_document(r#"[{"id":"b","name":"W48"},{"id":"a","name":"Project X"}]"#)
                .unwrap();
        let ids: Vec<_> = folders.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids, ["b", "a"]);
    }

    #[test]
    fn a_duplicate_id_is_refused() {
        let err =
            validate_document(r#"[{"id":"a","name":"x"},{"id":"a","name":"y"}]"#).unwrap_err();
        assert!(err.contains("twice"), "{err}");
    }

    #[test]
    fn a_blank_or_long_name_is_refused() {
        assert!(validate_document(r#"[{"id":"a","name":"  "}]"#).is_err());
        let long = "n".repeat(MAX_NAME_CHARS + 1);
        assert!(validate_document(&format!(r#"[{{"id":"a","name":"{long}"}}]"#)).is_err());
    }

    #[test]
    fn a_bad_id_is_refused() {
        assert!(validate_document(r#"[{"id":"A B","name":"x"}]"#).is_err());
        assert!(validate_document(r#"[{"id":"","name":"x"}]"#).is_err());
    }

    #[test]
    fn too_many_folders_are_refused() {
        let list: Vec<_> = (0..=MAX_FOLDERS)
            .map(|i| format!(r#"{{"id":"f{i}","name":"F{i}"}}"#))
            .collect();
        let err = validate_document(&format!("[{}]", list.join(","))).unwrap_err();
        assert!(err.contains(&MAX_FOLDERS.to_string()), "{err}");
    }

    #[test]
    fn the_description_states_every_bound() {
        let description = prefs::THREAD_FOLDERS.spec.description;
        for bound in [MAX_ID_CHARS, MAX_NAME_CHARS, MAX_FOLDERS] {
            assert!(description.contains(&bound.to_string()), "omits {bound}");
        }
    }
}
