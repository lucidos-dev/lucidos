//! The workspace order: the user's order for the workspace list, at
//! `<app-data>/config/workspace-order.json`.
//!
//! One list of *order keys*. A key is the route segment the workspace opens
//! at: `<slug>` for a local workspace, `<slug>@<gateway>` for a remote one. The
//! picker and the in-app switcher on every paired device draw the same order.
//! A workspace with no key keeps its default place after the ordered ones, so
//! an empty order is the default list.

use crate::BoxError;
use lucidos_gateway_constants::REMOTE_ROUTE_SEPARATOR;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// The most keys one order may hold. Far past any real workspace count, and
/// small enough that a hostile body cannot grow the file without bound.
pub const MAX_KEYS: usize = 1000;

pub fn workspace_order_path(app_data: &Path) -> PathBuf {
    app_data.join("config/workspace-order.json")
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceOrder {
    keys: Vec<String>,
}

impl WorkspaceOrder {
    /// `keys` as an order, or why not: too many, a malformed key, or a key
    /// listed twice.
    pub fn new(keys: Vec<String>) -> Result<Self, String> {
        if keys.len() > MAX_KEYS {
            return Err(format!("an order holds at most {MAX_KEYS} workspaces"));
        }
        let mut seen = std::collections::HashSet::new();
        for key in &keys {
            if !is_valid_key(key) {
                return Err(format!("{key:?} is not a workspace route segment"));
            }
            if !seen.insert(key) {
                return Err(format!("{key:?} is listed twice"));
            }
        }
        Ok(Self { keys })
    }

    pub fn keys(&self) -> &[String] {
        &self.keys
    }

    /// A missing file is the empty order, the state before any reorder.
    pub fn load(path: &Path) -> Result<Self, BoxError> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("read {}: {e}", path.display()).into()),
        };
        let stored: Self =
            serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", path.display()))?;
        Ok(Self::new(stored.keys).map_err(|e| format!("{}: {e}", path.display()))?)
    }

    pub fn save(&self, path: &Path) -> Result<(), BoxError> {
        crate::atomic_file::write(path, serde_json::to_string_pretty(self)?.as_bytes())
    }
}

/// `<slug>` or `<slug>@<gateway>`, each part a valid id.
fn is_valid_key(key: &str) -> bool {
    match key.split_once(REMOTE_ROUTE_SEPARATOR) {
        None => crate::registry::is_valid_id(key),
        Some((slug, gateway)) => {
            crate::registry::is_valid_id(slug)
                && crate::remote_gateways::valid_name(gateway).is_ok_and(|name| name == gateway)
        }
    }
}

/// The stored order with its file. Writes go to disk before memory, under one
/// lock, so a reader never sees an order the disk does not hold.
pub struct WorkspaceOrderStore {
    path: PathBuf,
    order: Mutex<WorkspaceOrder>,
}

impl WorkspaceOrderStore {
    /// An unreadable file starts the empty order rather than failing the
    /// gateway: an order is a preference, and the next save replaces the file.
    pub fn load(path: PathBuf) -> Self {
        let order = WorkspaceOrder::load(&path).unwrap_or_else(|e| {
            crate::log!("[Gateway] workspace order unreadable, starting empty: {e}");
            WorkspaceOrder::default()
        });
        Self {
            path,
            order: Mutex::new(order),
        }
    }

    pub fn keys(&self) -> Vec<String> {
        self.order.lock().unwrap().keys().to_vec()
    }

    pub fn replace(&self, order: WorkspaceOrder) -> Result<(), BoxError> {
        let mut current = self.order.lock().unwrap();
        order.save(&self.path)?;
        *current = order;
        Ok(())
    }

    /// Drop `key`, if the order holds it.
    pub fn forget(&self, key: &str) -> Result<(), BoxError> {
        let mut current = self.order.lock().unwrap();
        if !current.keys.iter().any(|k| k == key) {
            return Ok(());
        }
        let mut updated = current.clone();
        updated.keys.retain(|k| k != key);
        updated.save(&self.path)?;
        *current = updated;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|k| k.to_string()).collect()
    }

    #[test]
    fn a_missing_file_is_the_empty_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = workspace_order_path(dir.path());
        assert_eq!(
            WorkspaceOrder::load(&path).unwrap(),
            WorkspaceOrder::default()
        );
        assert!(WorkspaceOrderStore::load(path).keys().is_empty());
    }

    #[test]
    fn a_saved_order_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = workspace_order_path(dir.path());
        let order = WorkspaceOrder::new(keys(&["notes", "home@cloud", "dev"])).unwrap();
        WorkspaceOrderStore::load(path.clone())
            .replace(order.clone())
            .unwrap();
        assert_eq!(WorkspaceOrderStore::load(path).keys(), order.keys());
    }

    #[test]
    fn a_save_leaves_no_temp_file_beside_the_order() {
        let dir = tempfile::tempdir().unwrap();
        let path = workspace_order_path(dir.path());
        let store = WorkspaceOrderStore::load(path.clone());
        store
            .replace(WorkspaceOrder::new(keys(&["dev"])).unwrap())
            .unwrap();
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["workspace-order.json"]);
    }

    #[test]
    fn an_unreadable_file_starts_empty_and_the_next_save_replaces_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = workspace_order_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not json").unwrap();
        assert!(WorkspaceOrder::load(&path).is_err());
        let store = WorkspaceOrderStore::load(path.clone());
        assert!(store.keys().is_empty());
        store
            .replace(WorkspaceOrder::new(keys(&["dev"])).unwrap())
            .unwrap();
        assert_eq!(WorkspaceOrder::load(&path).unwrap().keys(), ["dev"]);
    }

    #[test]
    fn forgetting_a_key_drops_only_that_key_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = workspace_order_path(dir.path());
        let store = WorkspaceOrderStore::load(path.clone());
        store
            .replace(WorkspaceOrder::new(keys(&["a", "b", "b@cloud"])).unwrap())
            .unwrap();
        store.forget("b").unwrap();
        store.forget("never-listed").unwrap();
        assert_eq!(store.keys(), ["a", "b@cloud"]);
        assert_eq!(
            WorkspaceOrder::load(&path).unwrap().keys(),
            ["a", "b@cloud"]
        );
    }

    #[test]
    fn local_and_remote_keys_are_accepted() {
        assert!(WorkspaceOrder::new(keys(&["dev", "notes-2", "home@cloud"])).is_ok());
        assert!(WorkspaceOrder::new(Vec::new()).is_ok());
    }

    #[test]
    fn malformed_keys_are_refused() {
        for bad in [
            "", "Dev", "../etc", "~gateway", "a@", "@cloud", "a@b@c", "a@Cloud", " dev", "a@ cloud",
        ] {
            assert!(
                WorkspaceOrder::new(keys(&[bad])).is_err(),
                "{bad:?} must be refused"
            );
        }
        let long_gateway = format!("a@{}", "g".repeat(33));
        assert!(WorkspaceOrder::new(vec![long_gateway]).is_err());
    }

    #[test]
    fn a_key_listed_twice_is_refused() {
        assert!(WorkspaceOrder::new(keys(&["dev", "notes", "dev"])).is_err());
    }

    #[test]
    fn an_oversized_order_is_refused() {
        let at_limit: Vec<String> = (0..MAX_KEYS).map(|i| format!("w{i}")).collect();
        assert!(WorkspaceOrder::new(at_limit.clone()).is_ok());
        let mut over = at_limit;
        over.push("one-more".to_string());
        assert!(WorkspaceOrder::new(over).is_err());
    }
}
