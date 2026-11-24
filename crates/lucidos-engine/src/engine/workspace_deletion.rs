//! Confirmed batch deletion through the local gateway's workspace broker.

use super::gateway_control::{post_control, ControlRefusal, BROKER_DEADLINE};
use super::permission::batch::BatchItemOutcome;
use super::workspace_classification::MISSING_GATEWAY;
use crate::core::base_path::{gateway_port, workspace_id};

pub(super) async fn delete(workspaces: &[String], raiser: uuid::Uuid) -> BatchItemOutcome {
    let (Some(port), Some(own)) = (gateway_port(), workspace_id()) else {
        return BatchItemOutcome::Skipped {
            reason: MISSING_GATEWAY.into(),
        };
    };
    let body = serde_json::json!({
        "caller": { "workspace_id": own, "thread_id": raiser },
    });
    let mut deleted = 0;
    let mut uncertain = false;
    let mut failures = Vec::new();
    for slug in workspaces {
        if slug == &own {
            failures.push(format!(
                "`{slug}`: the broker refuses to delete the calling workspace"
            ));
            continue;
        }
        let path = format!("/broker/workspaces/{slug}/delete");
        let result = tokio::time::timeout(BROKER_DEADLINE, post_control(&port, &path, &body)).await;
        match result {
            Ok(Ok(_)) => deleted += 1,
            Ok(Err(ControlRefusal::Refused {
                message,
                unchanged: true,
                ..
            })) => {
                failures.push(format!("`{slug}`: {message}"));
            }
            Ok(Err(
                ControlRefusal::Refused { message, .. } | ControlRefusal::Unreachable(message),
            )) => {
                uncertain = true;
                failures.push(format!(
                    "`{slug}`: deletion did not report completion ({message})"
                ));
            }
            Err(_) => {
                uncertain = true;
                failures.push(format!(
                    "`{slug}`: the gateway timed out before reporting the result"
                ));
            }
        }
    }
    if failures.is_empty() {
        return BatchItemOutcome::Applied;
    }
    let reason = format!(
        "{deleted} of {} confirmed moved to Trash. {}",
        workspaces.len(),
        failures.join("; ")
    );
    if deleted == 0 && !uncertain {
        BatchItemOutcome::Skipped { reason }
    } else {
        BatchItemOutcome::PartlyApplied { reason }
    }
}
