//! Snapshot identity follows the gateway registry slug. An engine launched
//! without a gateway has no slug; its database name is its stable local identity.
//! The colon namespace cannot collide with gateway slugs (`[a-z0-9-]`).
use super::BoxError;

pub fn workspace_id(gateway_slug: Option<&str>, database: &str) -> Result<String, BoxError> {
    match gateway_slug {
        Some(slug) if lucidos_gateway_constants::is_valid_workspace_id(slug) => Ok(slug.to_owned()),
        Some(_) => Err("snapshot_workspace_identity_unavailable".into()),
        None if !database.is_empty() => Ok(format!("database:{database}")),
        None => Err("snapshot_workspace_identity_unavailable".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_identity_is_exact_and_bare_identity_is_stable_and_disjoint() {
        assert_eq!(
            workspace_id(Some("demo-workspace"), "demo_db").unwrap(),
            "demo-workspace"
        );
        assert_eq!(workspace_id(None, "demo_db").unwrap(), "database:demo_db");
        assert_eq!(
            workspace_id(None, "demo_db").unwrap(),
            workspace_id(None, "demo_db").unwrap()
        );
        assert_ne!(
            workspace_id(None, "demo_db").unwrap(),
            workspace_id(None, "another_db").unwrap()
        );
        for slug in [
            "",
            " Demo ",
            "../demo",
            "database:demo_db",
            "Demo",
            "demo/workspace",
            "-demo",
            "demo-",
            &"a".repeat(65),
        ] {
            assert_eq!(
                workspace_id(Some(slug), "demo_db").unwrap_err().to_string(),
                "snapshot_workspace_identity_unavailable"
            );
        }
        assert!(workspace_id(None, "").is_err());
    }
}
