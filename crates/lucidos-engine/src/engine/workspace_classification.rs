use super::gateway_control::{post_control, ControlRefusal};
use super::workspace_directory::{list_from_gateway, ListedWorkspace, GATEWAY_DEADLINE};

pub const MISSING_GATEWAY: &str =
    "This engine runs without a gateway, and the gateway keeps the workspaces.";

pub(crate) async fn unclassified_drift(workspaces: &[String]) -> Result<Option<String>, String> {
    let port = crate::core::base_path::gateway_port().ok_or_else(|| MISSING_GATEWAY.to_string())?;
    let listing = list_from_gateway(&port, GATEWAY_DEADLINE)
        .await
        .map_err(|e| e.message)?;
    Ok(classification_drift(&listing, workspaces))
}

pub fn classification_drift(listing: &[ListedWorkspace], workspaces: &[String]) -> Option<String> {
    workspaces
        .iter()
        .find_map(|slug| match listing.iter().find(|w| &w.id == slug) {
            None => Some(format!("workspace `{slug}` no longer exists")),
            Some(w) if w.origin != "unknown" => {
                Some(format!("workspace `{slug}` is already classified"))
            }
            Some(_) => None,
        })
}

pub(crate) struct ClassifyStop {
    pub(crate) moved: usize,
    pub(crate) reason: String,
}

pub(crate) async fn classify(
    workspaces: &[String],
    origin: crate::engine::permission::batch::ClassifiedOrigin,
) -> Result<(), ClassifyStop> {
    let port = crate::core::base_path::gateway_port().ok_or_else(|| ClassifyStop {
        moved: 0,
        reason: MISSING_GATEWAY.to_string(),
    })?;
    let body = serde_json::json!({ "origin": origin.wire() });
    for (moved, slug) in workspaces.iter().enumerate() {
        let path = format!("/broker/workspaces/{slug}/classify");
        let answer = tokio::time::timeout(GATEWAY_DEADLINE, post_control(&port, &path, &body))
            .await
            .unwrap_or_else(|_| {
                Err(ControlRefusal::Unreachable(
                    "the gateway did not answer in time".into(),
                ))
            });
        if let Err(refusal) = answer {
            let why = match refusal {
                ControlRefusal::Refused { message, .. } => message,
                ControlRefusal::Unreachable(why) => why,
            };
            return Err(ClassifyStop {
                moved,
                reason: format!(
                    "{moved} of {} classified, then `{slug}` was refused: {why}",
                    workspaces.len()
                ),
            });
        }
    }
    crate::log_legacy!(
        "[WorkspaceBroker] Classified {} workspaces as {}",
        workspaces.len(),
        origin.wire()
    );
    Ok(())
}
