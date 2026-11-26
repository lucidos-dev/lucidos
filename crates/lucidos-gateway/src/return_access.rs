use std::collections::HashMap;

use crate::error::ApiError;
use crate::gateway_protocol::AuthenticatedGateway;
use crate::server::GatewayState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

const RECEIPT_TTL_SECS: u64 = 30;
const MAX_PENDING_RECEIPTS: usize = 64;
const MAX_OPERATION_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReturnGrant {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReturnRoute {
    pub grant_id: String,
    pub gateway_id: String,
    pub address: String,
}

struct Pending {
    pairing: String,
    grant: String,
    operation: String,
    expires_at: u64,
}

#[derive(Default)]
pub(crate) struct Receipts(HashMap<String, Pending>);

impl Receipts {
    pub(crate) fn forget(&mut self, receipt: &str) {
        self.0.remove(receipt);
    }

    pub(crate) fn issue(
        &mut self,
        pairing: &str,
        grant: &str,
        operation: String,
        now: u64,
    ) -> Result<String, String> {
        self.0.retain(|_, pending| pending.expires_at > now);
        if self.0.len() >= MAX_PENDING_RECEIPTS || operation.len() > MAX_OPERATION_BYTES {
            return Err(
                "Too many pending return operations, or the operation is too large.".into(),
            );
        }
        let receipt = crate::auth::mint_credential().map_err(|e| e.to_string())?;
        self.0.insert(
            receipt.clone(),
            Pending {
                pairing: pairing.into(),
                grant: grant.into(),
                operation,
                expires_at: now.saturating_add(RECEIPT_TTL_SECS),
            },
        );
        Ok(receipt)
    }

    pub(crate) fn redeem(
        &mut self,
        receipt: &str,
        pairing: &str,
        grant: &str,
        now: u64,
    ) -> Option<String> {
        let pending = self.0.get(receipt)?;
        if pending.pairing != pairing || pending.grant != grant || pending.expires_at <= now {
            return None;
        }
        self.0.remove(receipt).map(|pending| pending.operation)
    }
}

pub(crate) async fn register_route(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
    Json(route): Json<Option<ReturnRoute>>,
) -> Result<StatusCode, ApiError> {
    if let Some(route) = &route {
        if !lucidos_gateway_stamp::is_plain_target(&route.grant_id)
            || !lucidos_gateway_stamp::is_plain_target(&route.gateway_id)
            || route.grant_id.len() > 256
            || route.gateway_id.len() > 256
            || route.gateway_id == state.gateway_id()
            || route.address.len() > 2048
        {
            return Err(ApiError::bad_request("Invalid return-route identity."));
        }
        crate::remote_gateways::valid_address(&route.address).map_err(ApiError::bad_request)?;
    }
    if !state
        .paired_gateways()
        .set_return_route(&gateway.pairing_id, route)
        .map_err(ApiError::internal)?
    {
        return Err(ApiError::unauthorized("This gateway is no longer paired."));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptRequest {
    pub grant_id: String,
}

pub(crate) async fn redeem_receipt(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
    Path(receipt): Path<String>,
    Json(request): Json<ReceiptRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let operation = state
        .paired_gateways()
        .with_return_receipts(&gateway.pairing_id, &request.grant_id, |receipts| {
            receipts.redeem(
                &receipt,
                &gateway.pairing_id,
                &request.grant_id,
                lucidos_gateway_stamp::now_unix(),
            )
        })
        .map_err(ApiError::forbidden)?
        .ok_or_else(|| ApiError::not_found("This return receipt is expired or already spent."))?;
    serde_json::from_str(&operation)
        .map(Json)
        .map_err(|error| ApiError::internal(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_local_process_cannot_enable_return_access_by_claiming_a_device() {
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;
        let dir = tempfile::tempdir().unwrap();
        let state = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
        let request = Request::put("/~/api/v1/remote-gateways/cloud/return-access")
            .header(crate::auth::HEADER_LOCAL_TOKEN, state.local_token())
            .header("x-lucidos-device-id", "claimed-device")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"enabled":true}"#))
            .unwrap();
        let response = crate::server::gateway_router(state)
            .oneshot(request)
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[test]
    fn a_receipt_is_single_use_and_bound_to_its_pairing_and_grant() {
        let mut receipts = Receipts::default();
        let receipt = receipts
            .issue("pair-a", "grant-a", "operation".into(), 100)
            .unwrap();
        assert!(receipts
            .redeem(&receipt, "pair-b", "grant-a", 101)
            .is_none());
        assert!(receipts
            .redeem(&receipt, "pair-a", "grant-b", 101)
            .is_none());
        assert_eq!(
            receipts.redeem(&receipt, "pair-a", "grant-a", 101),
            Some("operation".into())
        );
        assert!(receipts
            .redeem(&receipt, "pair-a", "grant-a", 101)
            .is_none());
    }

    #[test]
    fn receipts_expire_and_cannot_survive_restart() {
        let mut receipts = Receipts::default();
        let receipt = receipts
            .issue("pair-a", "grant-a", "operation".into(), 100)
            .unwrap();
        assert!(receipts
            .redeem(&receipt, "pair-a", "grant-a", 100 + RECEIPT_TTL_SECS)
            .is_none());
        assert!(Receipts::default()
            .redeem(&receipt, "pair-a", "grant-a", 101)
            .is_none());
    }

    #[test]
    fn receipt_capacity_is_bounded_without_evicting_live_operations() {
        let mut receipts = Receipts::default();
        for _ in 0..MAX_PENDING_RECEIPTS {
            receipts
                .issue("pair-a", "grant-a", "operation".into(), 100)
                .unwrap();
        }
        assert!(receipts
            .issue("pair-a", "grant-a", "operation".into(), 100)
            .is_err());
        assert!(receipts
            .issue(
                "pair-a",
                "grant-a",
                "operation".into(),
                100 + RECEIPT_TTL_SECS
            )
            .is_ok());
    }
}
