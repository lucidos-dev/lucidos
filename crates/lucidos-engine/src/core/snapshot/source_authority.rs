//! Target-owned source verification. A request body is never evidence of origin.
//! Approval spending uses the exact connection that verified the source; a lost
//! spend response fails closed and is never retried on another protocol.
use super::{handle::Handle, BoxError};
use serde::Deserialize;
use uuid::Uuid;

pub struct VerifiedSource {
    workspace: String,
    thread: Uuid,
    token: String,
    url: String,
    client: reqwest::Client,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proof {
    workspace_id: String,
    source_thread_id: Uuid,
}

impl VerifiedSource {
    pub fn workspace(&self) -> &str {
        &self.workspace
    }

    pub fn thread(&self) -> Uuid {
        self.thread
    }

    /// `target` must come from the target's validated registry binding.
    pub async fn verify(
        gateway_port: u16,
        source_hint: &str,
        token: &str,
        target: &super::service::OfflineRestoreRequest,
    ) -> Result<Self, BoxError> {
        if !lucidos_gateway_constants::is_valid_workspace_id(source_hint)
            || token.is_empty()
            || token.len() > 4096
        {
            return Err("snapshot_restore_caller_unverified".into());
        }
        let client = crate::gateway_auth::client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(std::time::Duration::from_secs(6))
            .build()
            .map_err(|_| "snapshot_restore_caller_unverified")?;
        let verify = async {
            for scheme in crate::net_config::peer_scheme_order() {
                let url = format!("{scheme}://127.0.0.1:{gateway_port}/~/api/v1/control/workspaces/{source_hint}/snapshot-restore-caller");
                let response = match client
                    .post(&url)
                    .json(&serde_json::json!({"origin_token": token}))
                    .send()
                    .await
                {
                    Ok(response) => response,
                    // Verification is read-only. Only connection/protocol failure
                    // permits the other scheme; a refusal is authoritative.
                    Err(_) => continue,
                };
                let proof = decode(response).await?;
                validate(&proof, source_hint, &target.gateway_slug)?;
                return Ok(Self {
                    workspace: proof.workspace_id,
                    thread: proof.source_thread_id,
                    token: token.to_owned(),
                    url,
                    client,
                });
            }
            Err("snapshot_restore_caller_unverified".into())
        };
        tokio::time::timeout(std::time::Duration::from_secs(6), verify)
            .await
            .map_err(|_| "snapshot_restore_caller_unverified")?
    }

    /// No retries: the source may have spent the card before a response is lost.
    pub async fn spend(
        &self,
        target: &super::service::OfflineRestoreRequest,
        handle: &Handle,
    ) -> Result<(), BoxError> {
        if self.workspace == target.gateway_slug {
            return Err("snapshot_restore_own_workspace_refused".into());
        }
        let response = self
            .client
            .post(&self.url)
            .json(&serde_json::json!({
                "origin_token": self.token,
                "approval": {"target_workspace_id": target.gateway_slug,
                             "snapshot_handle": handle.as_str()}
            }))
            .send()
            .await
            .map_err(|_| "snapshot_restore_approval_required")?;
        let proof = decode(response)
            .await
            .map_err(|_| "snapshot_restore_approval_required")?;
        validate(&proof, &self.workspace, &target.gateway_slug)?;
        if proof.source_thread_id != self.thread {
            return Err("snapshot_restore_approval_required".into());
        }
        Ok(())
    }
}

fn validate(proof: &Proof, source: &str, target: &str) -> Result<(), BoxError> {
    if proof.workspace_id != source || proof.source_thread_id.is_nil() {
        return Err("snapshot_restore_caller_unverified".into());
    }
    if proof.workspace_id == target {
        return Err("snapshot_restore_own_workspace_refused".into());
    }
    Ok(())
}

async fn decode(mut response: reqwest::Response) -> Result<Proof, BoxError> {
    if response.status() != reqwest::StatusCode::OK {
        return Err("snapshot_restore_caller_unverified".into());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "snapshot_restore_caller_unverified")?
    {
        if bytes.len() + chunk.len() > 4096 {
            return Err("snapshot_restore_caller_unverified".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "snapshot_restore_caller_unverified".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_requires_exact_source_and_real_thread_and_refuses_self() {
        let proof = Proof {
            workspace_id: "source".into(),
            source_thread_id: Uuid::new_v4(),
        };
        assert!(validate(&proof, "source", "target").is_ok());
        assert!(validate(&proof, "forged", "target").is_err());
        assert!(validate(&proof, "source", "source").is_err());
        assert!(validate(
            &Proof {
                source_thread_id: Uuid::nil(),
                ..proof
            },
            "source",
            "target"
        )
        .is_err());
        for thread in ["not-a-thread", "", "current"] {
            assert!(serde_json::from_value::<Proof>(serde_json::json!({
                "workspace_id": "source", "source_thread_id": thread
            }))
            .is_err());
        }
    }
    #[tokio::test]
    async fn spend_binds_exact_dimensions_and_never_retries_refusal_or_bad_proof() {
        use axum::{routing::post, Json, Router};
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        for outcome in ["ok", "refused", "changed-thread", "oversized", "redirect"] {
            let thread = Uuid::new_v4();
            let handle = Handle::generate();
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            let expected_handle = handle.as_str().to_owned();
            let app = Router::new().route("/source", post(move |Json(body): Json<serde_json::Value>| {
                let count = count.clone();
                let expected_handle = expected_handle.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    assert_eq!(body, serde_json::json!({
                        "origin_token": "private-origin-token",
                        "approval": {"target_workspace_id": "target", "snapshot_handle": expected_handle}
                    }));
                    match outcome {
                        "refused" => (axum::http::StatusCode::FORBIDDEN, "refused".to_owned()),
                        "redirect" => (axum::http::StatusCode::TEMPORARY_REDIRECT, String::new()),
                        "oversized" => (axum::http::StatusCode::OK, "x".repeat(4097)),
                        _ => (axum::http::StatusCode::OK, serde_json::json!({
                            "workspace_id": "source",
                            "source_thread_id": if outcome == "changed-thread" { Uuid::new_v4() } else { thread }
                        }).to_string()),
                    }
                }
            }));
            let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = socket.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
            let source = VerifiedSource {
                workspace: "source".into(),
                thread,
                token: "private-origin-token".into(),
                url: format!("{}://{address}/source", crate::net_config::SCHEME_HTTP),
                client: reqwest::Client::builder()
                    .no_proxy()
                    .redirect(reqwest::redirect::Policy::none())
                    .retry(reqwest::retry::never())
                    .build()
                    .unwrap(),
            };
            let mut target = super::super::service::OfflineRestoreRequest {
                restore_id: Uuid::new_v4(),
                gateway_slug: "source".into(),
                workspace_name: "Source".into(),
            };
            assert!(source.spend(&target, &handle).await.is_err());
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            target.gateway_slug = "target".into();
            assert_eq!(
                source.spend(&target, &handle).await.is_ok(),
                outcome == "ok"
            );
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            server.abort();
        }
    }
    #[tokio::test]
    async fn verify_calls_gateway_itself_and_binds_returned_identity() {
        use axum::{routing::post, Json, Router};
        for returned_source in ["source", "forged", "target"] {
            let thread = Uuid::new_v4();
            let app = Router::new().route(
                "/~/api/v1/control/workspaces/source/snapshot-restore-caller",
                post(move |Json(body): Json<serde_json::Value>| async move {
                    assert_eq!(body, serde_json::json!({"origin_token": "signed-token"}));
                    Json(serde_json::json!({"workspace_id": returned_source, "source_thread_id": thread}))
                }),
            );
            let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = socket.local_addr().unwrap().port();
            let server = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
            let target = super::super::service::OfflineRestoreRequest {
                restore_id: Uuid::new_v4(),
                gateway_slug: "target".into(),
                workspace_name: "Target".into(),
            };
            let result = VerifiedSource::verify(port, "source", "signed-token", &target).await;
            assert_eq!(result.is_ok(), returned_source == "source");
            if let Ok(source) = result {
                assert_eq!(source.workspace(), "source");
                assert_eq!(source.thread(), thread);
            }
            server.abort();
        }
    }
}
