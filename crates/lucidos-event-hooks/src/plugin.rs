use std::collections::BTreeSet;
use std::path::Path;

use lucidos_core::core::event_hooks::{HOOKS_DIR, HOOK_DEFINITION_FILE};
use lucidos_core::core::plugins::{plan_files, PlannedFile};
use lucidos_core::core::{is_path_traversal, slug::slugify_kebab};

use super::{contained, content_hash, definition_source, parse_definition};

pub(crate) fn bundle_scripts(root: &Path, planned: &mut Vec<PlannedFile>) -> Result<(), String> {
    for file in planned.iter() {
        let mut parts = file.data_relative.split('/');
        let (Some(HOOKS_DIR), Some(slug), Some(HOOK_DEFINITION_FILE), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if slugify_kebab(slug) != slug || slug.is_empty() {
            return Err(format!("Invalid hook folder name '{slug}'."));
        }
        let (dir, text) = definition_source(&root.join(HOOKS_DIR), slug)?;
        let definition = parse_definition(&text)?;
        let scripts = std::iter::once(&definition.script).chain(
            definition
                .arguments
                .values()
                .map(|argument| &argument.choices_script),
        );
        for script in scripts {
            if !script.starts_with("scripts/") || is_path_traversal(script) {
                continue;
            }
            let local = contained(&dir, script)?;
            if local.exists() {
                continue;
            }
            let source = planned
                .iter()
                .find(|file| &file.data_relative == script)
                .ok_or_else(|| {
                    format!("Hook '{slug}' could not find the shipped script '{script}'.")
                })?;
            if let Some(parent) = local.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::copy(&source.source, &local).map_err(|e| e.to_string())?;
        }
    }
    *planned = plan_files(root);
    let paths: BTreeSet<_> = planned
        .iter()
        .map(|file| file.data_relative.as_str())
        .collect();
    for path in &paths {
        let mut parts = path.split('/');
        let (Some(HOOKS_DIR), Some(slug), Some(HOOK_DEFINITION_FILE), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let prefix = format!("{HOOKS_DIR}/{slug}");
        for name in content_hash::file_names(&root.join(&prefix))? {
            if !paths.contains(format!("{prefix}/{name}").as_str()) {
                return Err(format!(
                    "Hook '{slug}' includes '{name}', which plugins cannot install."
                ));
            }
        }
    }
    Ok(())
}
