//! Child environment reachability checks shared by the real spawn builders.

use super::*;
use std::collections::BTreeMap;

const INTENTIONALLY_HIDDEN: &[(&str, &str)] = &[
    (
        "LUCIDOS_PRIVATE_DIR",
        "gateway-owned private boundary, not a script handle",
    ),
    (
        "LUCIDOS_GATEWAY_DATA",
        "gateway state; only workspaces are reopened",
    ),
    (
        "FASTEMBED_CACHE_DIR",
        "engine model cache, not a script handle",
    ),
];

fn unreachable(policy: &Policy, env: &BTreeMap<String, String>) -> Vec<String> {
    let mut failures = Vec::new();
    for (key, value) in env {
        if INTENTIONALLY_HIDDEN.iter().any(|(name, why)| {
            assert!(!why.is_empty());
            key.as_str() == *name
        }) {
            continue;
        }
        if value.starts_with("postgres://") || value.starts_with("postgresql://") {
            if let Some(socket) = database_socket(value).expect("valid database handle") {
                if policy.hides(&socket) && !policy.resolved().connectable_sockets.contains(&socket)
                {
                    failures.push(key.clone());
                }
            }
        } else {
            let paths = if key.ends_with("PATH") || key.ends_with("_DIRS") {
                std::env::split_paths(value).collect()
            } else {
                vec![PathBuf::from(value)]
            };
            if paths
                .iter()
                .any(|path| path.is_absolute() && policy.hides(path))
            {
                failures.push(key.clone());
            }
        }
    }
    failures
}

/// Exercise the builder's real values, then the packaged socket shape on every host.
pub(crate) fn assert_spawn_env_reachable(
    workspace: &Path,
    overrides: impl IntoIterator<Item = (String, String)>,
) {
    let overrides: BTreeMap<_, _> = overrides.into_iter().collect();
    assert!(overrides.contains_key(crate::core::LUCIDOS_DATABASE_URL));
    let mut env: BTreeMap<_, _> = std::env::vars().collect();
    env.extend(overrides);
    let url = &env[crate::core::LUCIDOS_DATABASE_URL];
    let policy = policy_for_database(
        workspace,
        database_socket(url).unwrap().map(VerifiedDatabaseSocket),
    );
    assert!(
        unreachable(&policy, &env).is_empty(),
        "unreachable handles: {:?}",
        unreachable(&policy, &env)
    );

    let hidden = policy.hidden.first().expect("private boundary");
    let mut url = lucidos_pg_url::PgUrl::parse(url).unwrap();
    url.host = lucidos_pg_url::Host::Socket(hidden.join("sandbox-env-probe"));
    let url = url.read_only_url();
    let socket = database_socket(&url).unwrap().unwrap();
    env.insert(crate::core::LUCIDOS_DATABASE_URL.to_string(), url);
    let policy = policy_for_database(workspace, Some(VerifiedDatabaseSocket(socket.clone())));
    assert!(
        policy.hides(&socket),
        "fixture must reproduce the hidden database socket"
    );
    assert!(
        unreachable(&policy, &env).is_empty(),
        "unreachable handles: {:?}",
        unreachable(&policy, &env)
    );
    let mut closed = policy.clone();
    closed.connectable_sockets.clear();
    assert!(unreachable(&closed, &env).contains(&crate::core::LUCIDOS_DATABASE_URL.to_string()));
    env.insert(
        "LUCIDOS_NEW_HANDLE".to_string(),
        hidden.join("new-handle").to_string_lossy().into_owned(),
    );
    assert!(unreachable(&policy, &env).contains(&"LUCIDOS_NEW_HANDLE".to_string()));
}
