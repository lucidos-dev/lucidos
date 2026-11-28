use std::collections::BTreeSet;
use std::path::Path;

use crate::core::event_hooks::{
    armed_hooks, ArmedHook, EventHooks, HookListing, HOOKS_DIR, HOOK_DEFINITION_FILE,
};
use crate::core::plugins::PlannedFile;
use crate::core::DATA_DIR;
use crate::engine::event_bus::SystemEvent;
use crate::engine::pending_plugins::{PluginHook, RecordedFiles};
use crate::engine::thread_events::MessageOrigin;

pub(crate) const CONFIRMATION_REQUIRED: &str =
    "The workspace owner must confirm plugin event hooks in the install or uninstall panel.";

pub(super) fn slug(path: &str) -> Option<&str> {
    let mut segments = path.split('/');
    match (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) {
        (Some(HOOKS_DIR), Some(slug), Some(HOOK_DEFINITION_FILE), None) => Some(slug),
        _ => None,
    }
}

pub(super) fn prepare(
    root: &Path,
    planned: &mut Vec<PlannedFile>,
    catalog: Option<&dyn EventHooks>,
) -> Result<Vec<PluginHook>, String> {
    let hooks = root.join(HOOKS_DIR);
    if !hooks.is_dir() {
        return Ok(Vec::new());
    }
    let catalog = catalog.ok_or("The event hook catalog is unavailable.")?;
    catalog.bundle_plugin_scripts(root, planned)?;
    inspect(&hooks, catalog)
}

pub(super) fn inspect(root: &Path, catalog: &dyn EventHooks) -> Result<Vec<PluginHook>, String> {
    catalog
        .list_at(root)?
        .into_iter()
        .map(|listing| match listing {
            HookListing::Valid {
                slug,
                definition,
                content_hash,
                ..
            } if catalog
                .asked_event_types()
                .contains(&definition.event_type.as_str()) =>
            {
                Ok(PluginHook {
                    slug,
                    name: definition.name,
                    event_type: definition.event_type,
                    content_hash,
                })
            }
            HookListing::Valid {
                slug, definition, ..
            } => Err(format!(
                "Hook '{slug}' cannot be armed. Lucidos does not ask about {} events yet.",
                definition.event_type
            )),
            HookListing::Invalid { slug, problems, .. } => {
                Err(format!("Invalid hook '{slug}': {}", problems.join(" ")))
            }
        })
        .collect()
}

pub(super) fn check_ownership(
    workspace: &Path,
    plugin_id: &str,
    staged: &[PluginHook],
    recorded: &RecordedFiles,
    armed: &[ArmedHook],
) -> Result<(), String> {
    for hook in staged {
        let others_own_hook = recorded.iter().any(|(id, paths)| {
            id != plugin_id
                && paths
                    .iter()
                    .any(|path| slug(path) == Some(hook.slug.as_str()))
        });
        let plugin_owns_hook = recorded.get(plugin_id).is_some_and(|paths| {
            paths
                .iter()
                .any(|path| slug(path) == Some(hook.slug.as_str()))
        });
        let path = workspace.join(DATA_DIR).join(HOOKS_DIR).join(&hook.slug);
        let hook_exists = match std::fs::symlink_metadata(&path) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => return Err(format!("Hook '{}' cannot be checked: {e}", hook.slug)),
        };
        if others_own_hook
            || (!plugin_owns_hook && (hook_exists || armed.iter().any(|a| a.slug == hook.slug)))
        {
            return Err(format!(
                "Plugin '{plugin_id}' cannot install hook '{}'. The slug belongs to another hook.",
                hook.slug
            ));
        }
    }
    Ok(())
}

pub(super) fn check_paths<'a>(
    workspace: &Path,
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    for path in paths {
        if path.split('/').next() != Some(HOOKS_DIR) {
            continue;
        }
        if !super::is_safe_data_path(path) {
            return Err(format!("Invalid hook path '{path}'."));
        }
        let mut target = workspace.join(DATA_DIR);
        let mut parts = path.split('/').peekable();
        while let Some(part) = parts.next() {
            target.push(part);
            match std::fs::symlink_metadata(&target) {
                Ok(metadata) if parts.peek().is_some() && metadata.is_dir() => {}
                Ok(metadata) if parts.peek().is_none() && metadata.is_file() => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                _ => {
                    return Err(format!(
                        "Hook path 'data/{path}' must use plain folders and files."
                    ))
                }
            }
        }
    }
    Ok(())
}

fn wants_arm(hook: &PluginHook, prior: &[PluginHook], armed: bool) -> bool {
    let Some(old) = prior.iter().find(|old| old.slug == hook.slug) else {
        return true;
    };
    old.event_type != hook.event_type || (old.content_hash != hook.content_hash && armed)
}

pub(super) fn removed(
    prior: &[PluginHook],
    staged: &[PluginHook],
    actor: Option<MessageOrigin>,
) -> Vec<SystemEvent> {
    let kept: BTreeSet<_> = staged.iter().map(|hook| hook.slug.as_str()).collect();
    prior
        .iter()
        .filter(|old| !kept.contains(old.slug.as_str()))
        .map(|old| SystemEvent::EventHookRemoved {
            slug: old.slug.clone(),
            actor: actor.clone(),
        })
        .collect()
}

pub(super) async fn load_armed(pool: &sqlx::PgPool) -> Result<Vec<ArmedHook>, String> {
    armed_hooks(pool)
        .await
        .map_err(|e| format!("The event hook armings could not be read: {e}"))
}

pub(super) struct Install<'a> {
    pub(super) plugin_id: &'a str,
    pub(super) catalog: &'a dyn EventHooks,
    pub(super) staged: &'a [PluginHook],
    pub(super) prior: &'a [PluginHook],
    pub(super) armed: Vec<ArmedHook>,
}

impl Install<'_> {
    pub(super) fn events(
        &self,
        workspace: &Path,
        actor: Option<MessageOrigin>,
    ) -> (Vec<SystemEvent>, Vec<String>) {
        let mut events = removed(self.prior, self.staged, actor.clone());
        let mut skipped = Vec::new();
        let live = std::cell::OnceCell::new();
        for hook in self.staged {
            let armed = self.armed.iter().find(|arming| arming.slug == hook.slug);
            let rearm = wants_arm(hook, self.prior, armed.is_some());
            let expected = if rearm {
                &hook.content_hash
            } else if let Some(armed) = armed {
                &armed.content_hash
            } else {
                continue;
            };
            let installed = live
                .get_or_init(|| self.catalog.list_definitions(workspace).unwrap_or_default())
                .iter()
                .find(|listing| listing.slug() == hook.slug)
                .and_then(HookListing::content_hash);
            if installed != Some(expected.as_str()) {
                skipped.push(hook.slug.clone());
            } else if rearm {
                events.push(SystemEvent::EventHookArmed {
                    slug: hook.slug.clone(),
                    event_type: hook.event_type.clone(),
                    content_hash: hook.content_hash.clone(),
                    actor: actor.clone(),
                });
            }
        }
        (events, skipped)
    }
}
