mod answer;
mod asker;
mod input;
mod json_strings;
mod plugin;
mod runner;
mod staged;

pub use answer::HookAnswer;
pub use asker::WorkspaceHookAsker;
pub use input::HookInput;
pub use lucidos_core::core::event_hooks::effective_timeout;
pub use lucidos_core::core::event_hooks::{PendingEvent, Refusal, RefusalCause, ReturnedEvent};
pub use runner::{run_hook, Confinement};

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use lucidos_core::core::contained_path::{join_contained, join_data_contained};
use lucidos_core::core::event_hooks::{
    EventHooks, HookDefinition, HookListing, HookedEventKind, ARMING_EVENT_TYPES,
    DEFAULT_TIMEOUT_SECS, HOOKS_DIR, HOOK_DEFINITION_FILE, REFUSED_EVENT_TYPE,
    TIMEOUT_CEILING_SECS,
};
use lucidos_core::core::event_subscription::{
    condition, known_names, validate_subscribable_event_type, SubscriptionSurface,
    SubscriptionVerdict,
};
use lucidos_core::core::slug::slugify_kebab;
use lucidos_core::core::{is_path_traversal, DATA_DIR};

mod content_hash;

pub struct WorkspaceHooks;

impl EventHooks for WorkspaceHooks {
    fn list_definitions(&self, workspace: &Path) -> Result<Vec<HookListing>, String> {
        self.list_at(&hooks_root(workspace)?)
    }

    fn bundle_plugin_scripts(
        &self,
        plugin_root: &Path,
        planned: &mut Vec<lucidos_core::core::plugins::PlannedFile>,
    ) -> Result<(), String> {
        plugin::bundle_scripts(plugin_root, planned)
    }

    fn list_at(&self, root: &Path) -> Result<Vec<HookListing>, String> {
        let entries = match std::fs::read_dir(root) {
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
                Ok(slug) => load(root, slug),
                Err(name) => not_utf8(name),
            })
            .collect())
    }
}

fn hooks_root(workspace: &Path) -> Result<PathBuf, String> {
    join_data_contained(&workspace.join(DATA_DIR), HOOKS_DIR)
        .ok_or_else(|| format!("{DATA_DIR}/{HOOKS_DIR} links outside {DATA_DIR}/"))
}

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

fn load(root: &Path, slug: String) -> HookListing {
    let mut problems = Vec::new();
    if slugify_kebab(&slug) != slug {
        problems.push(format!(
            "The folder name '{slug}' is not a kebab-case slug, such as 'require-a-plan'."
        ));
    }
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
    for (name, argument) in &definition.arguments {
        if name.is_empty() || slugify_kebab(name) != *name {
            problems.push(format!("arguments.{name}: use a kebab-case name."));
        }
        if argument.label.trim().is_empty() {
            problems.push(format!("arguments.{name}: label must not be blank."));
        }
        if argument
            .create_label
            .as_ref()
            .is_some_and(|label| label.trim().is_empty())
        {
            problems.push(format!("arguments.{name}: create_label must not be blank."));
        }
        if let Err(problem) = check_script(&dir, &argument.choices_script) {
            problems.push(format!("arguments.{name}.choices_script: {problem}"));
        }
    }
    if let Err(problem) = check_timeout(definition.timeout_secs) {
        problems.push(problem);
    }
    let content_hash = match event_kind {
        Some(_) if problems.is_empty() => content_hash::of(&dir, &text)
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

fn hooked_event_kind(event_type: &str) -> Result<HookedEventKind, String> {
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
    if event_type == REFUSED_EVENT_TYPE {
        return Err(format!(
            "event_type: '{event_type}' records a hook's refusal, and no hook is asked \
             about what a hook caused."
        ));
    }
    use lucidos_core::core::event_hooks::{APPLY_CHECK_EVENT, PROPOSAL_CHECK_EVENT};
    if matches!(event_type, PROPOSAL_CHECK_EVENT | APPLY_CHECK_EVENT) {
        return Ok(HookedEventKind::Thread);
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

fn check_script(hook_dir: &Path, script: &str) -> Result<PathBuf, String> {
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
    Ok(path)
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

fn contained(root: &Path, rel: &str) -> Result<PathBuf, String> {
    join_contained(root, rel).ok_or_else(|| format!("'{rel}' links outside its folder."))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
