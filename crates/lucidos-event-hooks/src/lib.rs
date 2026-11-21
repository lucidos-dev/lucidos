//! Event hooks (ADR 0746): the loader for `data/hooks/<slug>/hook.toml`.
//!
//! A leaf crate (ADR 0738). It implements the [`EventHooks`] interface from
//! `lucidos-core`, and the engine executable hands it to the router.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use lucidos_core::core::contained_path::{join_contained, join_data_contained};
use lucidos_core::core::event_hooks::{
    EventHooks, HookDefinition, HookListing, HookedEventKind, ARMING_EVENT_TYPES, HOOKS_DIR,
    HOOK_DEFINITION_FILE,
};
use lucidos_core::core::event_subscription::{
    condition, known_names, validate_subscribable_event_type, SubscriptionSurface,
    SubscriptionVerdict,
};
use lucidos_core::core::slug::slugify_kebab;
use lucidos_core::core::{is_path_traversal, DATA_DIR};

mod content_hash;

/// How long a hook may run when its definition names no timeout.
pub const DEFAULT_TIMEOUT_SECS: u64 = 10;

/// The engine-wide ceiling on any hook's timeout. A hook can block a person's
/// send, so one slow script must not stall the workspace for longer.
pub const TIMEOUT_CEILING_SECS: u64 = 60;

/// The hooks a workspace defines under its `data/hooks/` tree.
pub struct WorkspaceHooks;

impl EventHooks for WorkspaceHooks {
    fn list_definitions(&self, workspace: &Path) -> Result<Vec<HookListing>, String> {
        let root = hooks_root(workspace)?;
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(format!("{DATA_DIR}/{HOOKS_DIR} cannot be read: {e}")),
        };
        let mut names = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| format!("{DATA_DIR}/{HOOKS_DIR} cannot be read: {e}"))?;
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with('.') {
                names.push(name);
            }
        }
        names.sort();
        Ok(names
            .into_iter()
            .map(|name| match name.into_string() {
                Ok(slug) => load(&root, slug),
                Err(name) => not_utf8(name),
            })
            .collect())
    }
}

fn hooks_root(workspace: &Path) -> Result<PathBuf, String> {
    join_data_contained(&workspace.join(DATA_DIR), HOOKS_DIR)
        .ok_or_else(|| format!("{DATA_DIR}/{HOOKS_DIR} links outside {DATA_DIR}/"))
}

/// A folder name that is not UTF-8 names no path the loader can open again.
fn not_utf8(name: OsString) -> HookListing {
    HookListing::Invalid {
        slug: name.to_string_lossy().into_owned(),
        definition: None,
        problems: vec![
            "The folder name is not UTF-8, so it is not a kebab-case slug, such as \
             'require-a-plan'."
                .to_string(),
        ],
    }
}

/// Read and check one entry of the hooks tree.
fn load(root: &Path, slug: String) -> HookListing {
    let mut problems = Vec::new();
    if slugify_kebab(&slug) != slug {
        problems.push(format!(
            "The folder name '{slug}' is not a kebab-case slug, such as 'require-a-plan'."
        ));
    }
    // The digest hashes this same text, so it binds the definition listed.
    let source = definition_source(root, &slug)
        .and_then(|(dir, text)| Ok((dir, parse_definition(&text)?, text)));
    let (dir, definition, text) = match source {
        Ok(source) => source,
        Err(problem) => {
            problems.push(problem);
            return HookListing::Invalid {
                slug,
                definition: None,
                problems,
            };
        }
    };
    if definition.name.trim().is_empty() {
        problems.push("name must not be blank.".to_string());
    }
    let event_kind = hooked_event_kind(&definition.event_type)
        .map_err(|problem| problems.push(problem))
        .ok();
    if let Some(Err(e)) = definition.condition.as_ref().map(condition::validate) {
        problems.push(format!("condition: {e}"));
    }
    if let Err(problem) = check_script(&dir, &definition.script) {
        problems.push(problem);
    }
    if let Err(problem) = check_timeout(definition.timeout_secs) {
        problems.push(problem);
    }
    let content_hash = match event_kind {
        Some(_) if problems.is_empty() => content_hash::of(&dir, &text, &definition.script)
            .map_err(|problem| problems.push(problem))
            .ok(),
        _ => None,
    };
    match (event_kind, content_hash) {
        (Some(event_kind), Some(content_hash)) => HookListing::Valid {
            slug,
            event_kind,
            definition,
            content_hash,
        },
        _ => HookListing::Invalid {
            slug,
            definition: Some(definition),
            problems,
        },
    }
}

/// The hook's folder and the text of its definition file.
fn definition_source(root: &Path, slug: &str) -> Result<(PathBuf, String), String> {
    let dir = contained(root, slug)?;
    let metadata = std::fs::metadata(&dir).map_err(|e| format!("'{slug}' cannot be read: {e}"))?;
    if !metadata.is_dir() {
        return Err(format!(
            "A hook is a folder holding {HOOK_DEFINITION_FILE}, not a file."
        ));
    }
    let file = contained(&dir, HOOK_DEFINITION_FILE)?;
    let text = std::fs::read_to_string(&file)
        .map_err(|e| format!("{HOOK_DEFINITION_FILE} cannot be read: {e}"))?;
    Ok((dir, text))
}

fn parse_definition(text: &str) -> Result<HookDefinition, String> {
    toml::from_str(text).map_err(|e| format!("{HOOK_DEFINITION_FILE} does not parse: {e}"))
}

/// The carrier of an event a hook may name. A hook is asked before its event
/// is recorded, so it names a recorded thread or system event, and nothing
/// else.
fn hooked_event_kind(event_type: &str) -> Result<HookedEventKind, String> {
    // The check below trims, but the definition keeps the name as written and
    // is later matched against an event's exact type.
    if event_type.trim() != event_type {
        return Err(format!(
            "event_type: '{event_type}' must not start or end with a space."
        ));
    }
    if ARMING_EVENT_TYPES.contains(&event_type) {
        return Err(format!(
            "event_type: '{event_type}' is how the owner arms, disarms or removes a hook, \
             and no hook can stop that."
        ));
    }
    match validate_subscribable_event_type(event_type, SubscriptionSurface::Trigger) {
        Ok(SubscriptionVerdict::KnownThreadEvent) => Ok(HookedEventKind::Thread),
        Ok(SubscriptionVerdict::KnownSystemEvent) => Ok(HookedEventKind::System),
        Ok(SubscriptionVerdict::UnknownName) => Err(format!(
            "event_type: '{event_type}' is not an event Lucidos records. A hook names a \
             thread event or a persisted system event.{}",
            known_names::did_you_mean(event_type)
        )),
        Err(e) => Err(format!("event_type: {e}")),
    }
}

fn check_script(hook_dir: &Path, script: &str) -> Result<(), String> {
    if script.is_empty() || is_path_traversal(script) {
        return Err(format!(
            "script: '{script}' must be a path inside the hook's folder."
        ));
    }
    lucidos_engine::triggers::validate_script_extension(script)
        .map_err(|e| format!("script: {e}"))?;
    let path = contained(hook_dir, script)?;
    if !path.is_file() {
        return Err(format!(
            "script: '{script}' is not a file in the hook's folder."
        ));
    }
    Ok(())
}

fn check_timeout(timeout_secs: Option<u64>) -> Result<(), String> {
    match timeout_secs {
        Some(secs) if !(1..=TIMEOUT_CEILING_SECS).contains(&secs) => Err(format!(
            "timeout_secs must be between 1 and {TIMEOUT_CEILING_SECS}, the engine-wide \
             ceiling. Leave it out for {DEFAULT_TIMEOUT_SECS}."
        )),
        _ => Ok(()),
    }
}

/// `root.join(rel)`, refused when a symlink carries it out of `root`.
fn contained(root: &Path, rel: &str) -> Result<PathBuf, String> {
    join_contained(root, rel).ok_or_else(|| format!("'{rel}' links outside its folder."))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
