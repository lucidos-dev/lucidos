//! Read wire keys from the engine catalog without linking the engine into the gateway.

pub(crate) fn key(handle: &str) -> &'static str {
    let catalog = include_str!("../../lucidos-engine/src/core/preference_catalog.rs");
    let declaration = format!("pub const {handle}: Pref<");
    let (_, rest) = catalog
        .split_once(&declaration)
        .expect("catalog handle exists");
    let (_, fields) = rest.split_once("PrefSpec {").expect("handle has a spec");
    let spec = fields.split_once("});").expect("spec ends").0;
    let key = spec
        .lines()
        .map(str::trim)
        .find_map(|line| {
            line.strip_prefix("key: \"")
                .and_then(|key| key.strip_suffix("\","))
        })
        .expect("spec has a literal wire key");
    assert!(!key.is_empty());
    key
}

#[test]
fn gateway_fixture_keys_come_from_the_catalog() {
    let path = key("CODING_AGENT_CLAUDE_PATH");
    let policy = key("SNAPSHOT_RESTORE_BY_AGENTS");
    assert_ne!(path, policy);
    assert!(path.ends_with("_path"));
    assert!(policy.ends_with("_by_agents"));
}
