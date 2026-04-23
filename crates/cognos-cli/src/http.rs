use crate::workspace::BoxError;

/// Blocking HTTP client preconfigured for the local CognOS engine. Accepts
/// the engine's self-signed cert because the target is always `localhost`.
pub(crate) fn client() -> Result<reqwest::blocking::Client, BoxError> {
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e).into())
}
