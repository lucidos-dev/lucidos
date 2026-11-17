//! Canonical exact mint targets and typed creator-bound scopes. No wildcard matching.
use super::GrantBreadth;

pub(crate) fn decode_target(target: &str) -> Option<(String, String, u64)> {
    let (workspace, device, ttl): (String, String, u64) = serde_json::from_str(target).ok()?;
    (lucidos_gateway_constants::is_valid_workspace_id(&workspace)
        && lucidos_gateway_stamp::is_valid_session_device_id(&device)
        && ttl > 0
        && ttl <= lucidos_gateway_stamp::MAX_SESSION_SECS)
        .then_some((workspace, device, ttl))
}

impl GrantBreadth {
    pub(crate) fn device_session(source: &str, target: &str) -> Option<Self> {
        let (_, device_id, ttl_secs) = decode_target(target)?;
        lucidos_gateway_constants::is_valid_workspace_id(source).then(|| Self::DeviceSession {
            creator_workspace: source.into(),
            device_id,
            ttl_secs,
        })
    }

    pub(crate) fn matches_device_target(&self, target: Option<&str>) -> bool {
        let Some((_, device, ttl)) = target.and_then(decode_target) else {
            return false;
        };
        matches!(self, Self::DeviceSession { device_id, ttl_secs, .. }
            if device_id == &device && *ttl_secs == ttl && self.device_scope_key().is_some())
    }

    pub(crate) fn device_scope_key(&self) -> Option<String> {
        let Self::DeviceSession {
            creator_workspace,
            device_id,
            ttl_secs,
        } = self
        else {
            return None;
        };
        let target = serde_json::json!([creator_workspace, device_id, ttl_secs]).to_string();
        decode_target(&target)?;
        serde_json::to_string(self).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_and_unsupported_scopes_fail_closed() {
        for value in [
            serde_json::json!({"device-session": {"creator_workspace": "../bad", "device_id": "page", "ttl_secs": 120}}),
            serde_json::json!({"device-session": {"creator_workspace": "source", "device_id": "page", "ttl_secs": 0}}),
            serde_json::json!({"device-session": {"creator_workspace": "source", "device_id": "", "ttl_secs": 120}}),
        ] {
            let scope: GrantBreadth = serde_json::from_value(value).unwrap();
            assert!(scope.device_scope_key().is_none());
            assert!(!scope.matches_device_target(Some(r#"["target","page",120]"#)));
        }
        assert!(
            serde_json::from_value::<GrantBreadth>(serde_json::json!({"future-scope": {}}))
                .is_err()
        );
        let scope = GrantBreadth::device_session("source", r#"["target","page",120]"#).unwrap();
        assert!(scope.matches_device_target(Some(r#"["future-target","page",120]"#)));
        assert!(!scope.matches_device_target(None));
        assert!(!scope.matches_device_target(Some("malformed")));
    }
}
