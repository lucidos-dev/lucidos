use super::*;
use crate::api::actor::{owner_attestation, require_attested_owner, OwnerAttestation};
use crate::api::gateway_stamp::test_support::stamped_device_headers;
use crate::core::subprocess_origin::{
    init_agent_origin_secret, mint_agent_origin_token, HEADER_AGENT_ORIGIN_TOKEN, HEADER_DEVICE_ID,
};
use axum::http::HeaderMap;
use uuid::Uuid;

fn typed_device() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_DEVICE_ID, "owner-phone".parse().unwrap());
    headers
}

fn local_token() -> HeaderMap {
    let token = crate::api::local_auth::publish_test_local_token();
    let mut headers = HeaderMap::new();
    headers.insert(
        lucidos_local_token::HEADER_LOCAL_TOKEN,
        token.parse().unwrap(),
    );
    headers
}

fn agent() -> HeaderMap {
    init_agent_origin_secret("owner-door-test-secret".to_string());
    let token = mint_agent_origin_token(Some(Uuid::new_v4()), 0, None).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(HEADER_AGENT_ORIGIN_TOKEN, token.parse().unwrap());
    headers
}

#[test]
fn attestation_reads_each_credential() {
    assert_eq!(
        owner_attestation(&stamped_device_headers("owner-phone")),
        OwnerAttestation::Owner
    );
    assert_eq!(owner_attestation(&agent()), OwnerAttestation::Agent);
    for unattested in [typed_device(), local_token(), HeaderMap::new()] {
        assert_eq!(
            owner_attestation(&unattested),
            OwnerAttestation::Unattested,
            "{unattested:?}"
        );
    }
}

#[test]
fn a_door_refuses_unattested_callers_with_the_same_guidance() {
    for headers in [typed_device(), local_token(), HeaderMap::new()] {
        let refused = require_attested_owner(&headers, "agents may not")
            .expect_err("an unattested caller is refused");
        assert_eq!(refused.status, StatusCode::FORBIDDEN);
        assert_eq!(refused.message, NOT_ATTESTED_AS_OWNER);
        assert!(refused.message.contains("Lucidos app"));
        assert!(refused.message.contains("trigger"));
    }
    let refused = require_attested_owner(&agent(), "agents may not")
        .expect_err("an agent hears its own refusal");
    assert_eq!(refused.message, "agents may not");
    require_attested_owner(&stamped_device_headers("owner-phone"), "agents may not")
        .expect("the stamped device is the owner");
}
