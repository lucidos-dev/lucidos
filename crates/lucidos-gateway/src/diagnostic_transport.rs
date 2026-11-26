use crate::{
    error::ApiError,
    gateway_protocol::{AuthenticatedGateway, HEADER_GATEWAY_CREDENTIAL},
    server::GatewayState,
};
use axum::{
    extract::{Path, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension,
};
use lucidos_gateway_stamp::{
    DiagnosticDelivery, StampKey, HEADER_DIAGNOSTIC_DELIVERY, HEADER_DIAGNOSTIC_DIGEST,
    HEADER_DIAGNOSTIC_ORIGIN, HEADER_GATEWAY_STAMP,
};

const HEADER_SIGNATURE: &str = "x-lucidos-diagnostic-relay-signature";

#[derive(Debug, Clone)]
pub(crate) struct AttestedDiagnosticDelivery(DiagnosticDelivery);

impl AttestedDiagnosticDelivery {
    pub(crate) fn forwarded_headers(
        &self,
        target: &str,
        key: Option<&StampKey>,
    ) -> Vec<(&'static str, String)> {
        let mut headers = self.0.headers();
        if let Some(key) = key {
            headers.push((HEADER_GATEWAY_STAMP, key.sign_now(self.0.claim(target))));
        }
        headers
    }
}

pub(crate) fn owns_header(name: &str) -> bool {
    [
        HEADER_DIAGNOSTIC_ORIGIN,
        HEADER_DIAGNOSTIC_DELIVERY,
        HEADER_DIAGNOSTIC_DIGEST,
        HEADER_SIGNATURE,
    ]
    .iter()
    .any(|owned| name.eq_ignore_ascii_case(owned))
}

fn claimed(headers: &HeaderMap) -> Option<DiagnosticDelivery> {
    let get = |name| headers.get(name).and_then(|v| v.to_str().ok());
    DiagnosticDelivery::new(
        get(HEADER_DIAGNOSTIC_ORIGIN)?,
        get(HEADER_DIAGNOSTIC_DELIVERY)?,
        get(HEADER_DIAGNOSTIC_DIGEST)?,
    )
}

fn relayed_text(delivery: &DiagnosticDelivery, target: &str, issued_at: i64) -> String {
    [
        "diagnostic-delivery/v1",
        &issued_at.to_string(),
        target,
        delivery.origin(),
        delivery.delivery_key(),
        delivery.body_digest(),
    ]
    .map(|field| urlencoding::encode(field).into_owned())
    .join("|")
}

fn verified_relay(
    target: &str,
    pairing_id: &str,
    headers: &HeaderMap,
    credential: &str,
    now: i64,
) -> Option<AttestedDiagnosticDelivery> {
    let delivery = claimed(headers)?;
    let (issued, mac) = headers
        .get(HEADER_SIGNATURE)?
        .to_str()
        .ok()?
        .split_once('.')?;
    let issued = issued.parse::<i64>().ok()?;
    if now.abs_diff(issued) > crate::relay::CLAIM_MAX_SKEW_SECS {
        return None;
    }
    if !crate::auth::ct_eq(
        mac,
        &crate::relay::sign(credential, &relayed_text(&delivery, target, issued)),
    ) {
        return None;
    }
    use sha2::{Digest, Sha256};
    let identity = serde_json::to_vec(&(pairing_id, delivery.origin())).ok()?;
    let origin = format!("paired:{:x}", Sha256::digest(identity));
    DiagnosticDelivery::new(&origin, delivery.delivery_key(), delivery.body_digest())
        .map(AttestedDiagnosticDelivery)
}

pub(crate) async fn receive(
    State(state): State<GatewayState>,
    Extension(gateway): Extension<AuthenticatedGateway>,
    Path(target): Path<String>,
    mut req: Request,
) -> Response {
    let credential = req
        .headers()
        .get(HEADER_GATEWAY_CREDENTIAL)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let Some(delivery) = verified_relay(
        &target,
        &gateway.pairing_id,
        req.headers(),
        credential,
        chrono::Utc::now().timestamp(),
    ) else {
        return ApiError::unauthorized("The diagnostic delivery signature does not verify")
            .into_response();
    };
    let Some(id) = state.workspace_call_target(&target) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(uri) = format!("/{id}/api/v1/diagnostic-bundles").parse() else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    *req.uri_mut() = uri;
    req.extensions_mut()
        .remove::<crate::auth::AuthenticatedDevice>();
    req.extensions_mut().insert(delivery);
    crate::server::serve_workspace(&state, &id, &format!("/{id}/"), req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn headers() -> HeaderMap {
        let delivery = DiagnosticDelivery::new("source", "once", &"a".repeat(64)).unwrap();
        let mut headers = HeaderMap::new();
        for (name, value) in delivery.headers() {
            headers.insert(name, value.parse().unwrap());
        }
        let signature = crate::relay::sign("credential", &relayed_text(&delivery, "target", 1000));
        headers.insert(
            HEADER_SIGNATURE,
            format!("1000.{signature}").parse().unwrap(),
        );
        headers
    }

    #[test]
    fn relay_signature_binds_diagnostic_delivery_and_expires() {
        assert!(verified_relay("target", "pairing-a", &headers(), "credential", 1000).is_some());
        for (target, credential, now) in [
            ("other", "credential", 1000),
            ("target", "wrong", 1000),
            ("target", "credential", 2000),
        ] {
            assert!(verified_relay(target, "pairing-a", &headers(), credential, now).is_none());
        }
        for (name, value) in [
            (HEADER_DIAGNOSTIC_ORIGIN, "other".into()),
            (HEADER_DIAGNOSTIC_DELIVERY, "twice".into()),
            (HEADER_DIAGNOSTIC_DIGEST, "b".repeat(64)),
        ] {
            let mut headers = headers();
            headers.insert(name, value.parse().unwrap());
            assert!(verified_relay("target", "pairing-a", &headers, "credential", 1000).is_none());
        }
    }

    #[test]
    fn relay_origins_are_scoped_to_the_authenticated_pairing() {
        let a = verified_relay("target", "pairing-a", &headers(), "credential", 1000).unwrap();
        let retry = verified_relay("target", "pairing-a", &headers(), "credential", 1000).unwrap();
        let b = verified_relay("target", "pairing-b", &headers(), "credential", 1000).unwrap();
        assert_eq!(a.0.origin(), retry.0.origin());
        assert_ne!(a.0.origin(), b.0.origin());
        assert_ne!(a.0.origin(), "source");
        assert_eq!(a.0.delivery_key(), "once");
        assert_eq!(a.0.body_digest(), "a".repeat(64));
    }

    #[test]
    fn proxy_stamp_is_for_target_and_never_a_chat_claim() {
        let delivery =
            verified_relay("target", "pairing-a", &headers(), "credential", 1000).unwrap();
        let key = StampKey::generate().unwrap();
        let headers = delivery.forwarded_headers("target-id", Some(&key));
        let stamp = &headers
            .iter()
            .find(|(name, _)| *name == HEADER_GATEWAY_STAMP)
            .unwrap()
            .1;
        assert!(key.verify_now(stamp, delivery.0.claim("target-id")));
        assert!(!key.verify_now(stamp, delivery.0.claim("target")));
        assert!(!key.verify_now(
            stamp,
            lucidos_gateway_stamp::Claim::WorkspaceCall {
                target_workspace: "target-id",
                caller_workspace: "source",
                caller_thread_id: "once",
                verdict: delivery.0.body_digest(),
            }
        ));
    }
}
