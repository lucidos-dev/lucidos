//! Refusals at owner-only routes.

use super::actor::NOT_ATTESTED_AS_OWNER;
use super::error::ApiError;
use axum::http::StatusCode;

pub(crate) fn unattested() -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, NOT_ATTESTED_AS_OWNER).with_reason("not_attested_as_owner")
}

/// Card answers stay the owner's own, so an unattested caller never answers.
pub const UNATTESTED_CARD_ANSWER: &str =
    "A card in Lucidos is answered by the workspace owner, and this request does not prove \
     it came from them. A device id or the machine-local token is a claim any local process \
     can make. Answer the card in the Lucidos app instead.";

#[cfg(test)]
#[path = "owner_door_tests.rs"]
mod tests;
