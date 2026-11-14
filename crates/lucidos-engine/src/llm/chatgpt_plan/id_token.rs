//! Verifying the sign-in's `id_token`: RS256 against the auth host's JWKS,
//! with the issuer, audience and `nonce` checked (OIDC Core § 3.1.3.7).
//!
//! The issuer and the key set URL come from the auth host's discovery
//! document, never from a guess.

use std::collections::HashSet;

use base64::Engine as _;
use jwt_simple::prelude::{RS256PublicKey, RSAPublicKeyLike as _, Token, VerificationOptions};
use serde::Deserialize;

use super::auth_server::{http_client, AuthServer};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The signed-in ChatGPT account, read from a verified `id_token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Identity {
    pub issuer: String,
    pub subject: String,
    /// Only displayed, never used to decide anything.
    pub email: Option<String>,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
}

#[derive(Deserialize)]
struct KeySet {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

#[derive(Deserialize, serde::Serialize)]
struct ProfileClaims {
    #[serde(default)]
    email: Option<String>,
}

/// Verify `id_token`, issued to `client_id` for the sign-in that sent
/// `nonce`, and return the account it names.
pub(super) async fn verify(
    auth: &AuthServer,
    id_token: &str,
    client_id: &str,
    nonce: &str,
) -> Result<Identity, BoxError> {
    let discovery: Discovery = get_json(&auth.discovery_url()).await?;
    if discovery.issuer.trim_end_matches('/') != auth.origin() {
        return Err(format!(
            "the discovery document names issuer {}, not {}",
            discovery.issuer,
            auth.origin()
        )
        .into());
    }
    if !discovery
        .jwks_uri
        .starts_with(&format!("{}/", auth.origin()))
    {
        return Err(format!("the key set {} is not on the auth host", discovery.jwks_uri).into());
    }
    let keys: KeySet = get_json(&discovery.jwks_uri).await?;
    verify_with(id_token, &keys, &discovery.issuer, client_id, nonce)
}

async fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, BoxError> {
    let resp = http_client().get(url).send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("{url} answered {status}").into());
    }
    Ok(resp.json().await?)
}

fn verify_with(
    id_token: &str,
    keys: &KeySet,
    issuer: &str,
    client_id: &str,
    nonce: &str,
) -> Result<Identity, BoxError> {
    let metadata =
        Token::decode_metadata(id_token).map_err(|e| format!("the ID token is malformed: {e}"))?;
    if metadata.algorithm() != "RS256" {
        return Err(format!("the ID token is signed {}, not RS256", metadata.algorithm()).into());
    }
    let key = signing_key(keys, metadata.key_id())?;
    let options = VerificationOptions {
        allowed_issuers: Some(HashSet::from([issuer.to_string()])),
        allowed_audiences: Some(HashSet::from([client_id.to_string()])),
        required_nonce: Some(nonce.to_string()),
        ..VerificationOptions::default()
    };
    let claims = key
        .verify_token::<ProfileClaims>(id_token, Some(options))
        .map_err(|e| format!("the ID token did not verify: {e}"))?;
    if claims.expires_at.is_none() {
        return Err("the ID token has no expiry".into());
    }
    let subject = claims
        .subject
        .filter(|s| !s.is_empty())
        .ok_or("the ID token names no account")?;
    Ok(Identity {
        issuer: issuer.to_string(),
        subject,
        email: claims
            .custom
            .email
            .map(|email| email.trim().to_string())
            .filter(|email| !email.is_empty()),
    })
}

/// The RSA key `kid` names. A token with no `kid` takes the only RSA key.
fn signing_key(keys: &KeySet, kid: Option<&str>) -> Result<RS256PublicKey, BoxError> {
    let mut rsa = keys.keys.iter().filter(|key| key.kty == "RSA");
    let key = match kid {
        Some(kid) => rsa.find(|key| key.kid.as_deref() == Some(kid)),
        None => rsa.next().filter(|_| rsa.next().is_none()),
    }
    .ok_or("no key in the auth host's key set signed the ID token")?;
    let component = |value: &Option<String>| {
        value
            .as_deref()
            .and_then(|v| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(v.trim_end_matches('='))
                    .ok()
            })
            .ok_or("a key in the auth host's key set is malformed")
    };
    Ok(RS256PublicKey::from_components(
        &component(&key.n)?,
        &component(&key.e)?,
    )?)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use jwt_simple::prelude::{Claims, Duration, RS256KeyPair, RSAKeyPairLike as _};

    /// One signing key for every test: RSA key generation is slow.
    pub(crate) fn test_key() -> &'static RS256KeyPair {
        static KEY: std::sync::LazyLock<RS256KeyPair> = std::sync::LazyLock::new(|| {
            RS256KeyPair::generate(2048)
                .expect("test RSA key")
                .with_key_id("test-key")
        });
        &KEY
    }

    /// The JWKS a test auth host serves for [`test_key`].
    pub(crate) fn test_jwks() -> serde_json::Value {
        let components = test_key().public_key().to_components();
        let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        serde_json::json!({"keys": [{
            "kty": "RSA",
            "kid": "test-key",
            "alg": "RS256",
            "use": "sig",
            "n": encode(&components.n),
            "e": encode(&components.e),
        }]})
    }

    /// An `id_token` signed with [`test_key`], shaped like OpenAI's: `aud`
    /// is an array holding the client id.
    pub(crate) fn signed_id_token(issuer: &str, client_id: &str, nonce: &str) -> String {
        signed_id_token_for("user-123", issuer, client_id, nonce)
    }

    /// [`signed_id_token`] for the account `subject`.
    pub(crate) fn signed_id_token_for(
        subject: &str,
        issuer: &str,
        client_id: &str,
        nonce: &str,
    ) -> String {
        let claims = Claims::with_custom_claims(
            ProfileClaims {
                email: Some("user@example.com".into()),
            },
            Duration::from_hours(1),
        )
        .with_issuer(issuer)
        .with_subject(subject)
        .with_audiences(HashSet::from([client_id.to_string()]))
        .with_nonce(nonce);
        test_key().sign(claims).expect("sign the test ID token")
    }

    fn keys() -> KeySet {
        serde_json::from_value(test_jwks()).unwrap()
    }

    const ISSUER: &str = "https://auth.openai.com";

    #[test]
    fn a_token_for_this_sign_in_names_its_account() {
        let token = signed_id_token(ISSUER, "oaiapp_1", "n-1");
        let identity = verify_with(&token, &keys(), ISSUER, "oaiapp_1", "n-1").unwrap();
        assert_eq!(
            identity,
            Identity {
                issuer: ISSUER.into(),
                subject: "user-123".into(),
                email: Some("user@example.com".into()),
            }
        );
    }

    #[test]
    fn a_token_for_another_sign_in_is_refused() {
        let token = signed_id_token(ISSUER, "oaiapp_1", "n-1");
        for (issuer, client_id, nonce) in [
            ("https://auth.example", "oaiapp_1", "n-1"),
            (ISSUER, "oaiapp_other", "n-1"),
            (ISSUER, "oaiapp_1", "n-other"),
        ] {
            assert!(
                verify_with(&token, &keys(), issuer, client_id, nonce).is_err(),
                "{issuer} {client_id} {nonce}"
            );
        }
    }

    #[test]
    fn a_forged_or_unsigned_token_is_refused() {
        let token = signed_id_token(ISSUER, "oaiapp_1", "n-1");
        let mut parts: Vec<&str> = token.split('.').collect();
        let other = signed_id_token("https://auth.example", "oaiapp_1", "n-1");
        let other_payload = other.split('.').nth(1).unwrap();
        parts[1] = other_payload;
        let forged = parts.join(".");
        assert!(verify_with(&forged, &keys(), "https://auth.example", "oaiapp_1", "n-1").is_err());
        let encode = |v: serde_json::Value| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string())
        };
        let unsigned = format!(
            "{}.{}.",
            encode(serde_json::json!({"alg": "none"})),
            encode(
                serde_json::json!({"iss": ISSUER, "sub": "u", "aud": ["oaiapp_1"], "nonce": "n-1"})
            ),
        );
        assert!(verify_with(&unsigned, &keys(), ISSUER, "oaiapp_1", "n-1").is_err());
    }
}
