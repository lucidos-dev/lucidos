//! Keeps the proxy's injected credentials out of what it hands back.
//!
//! The proxy injects a credential so the caller never holds it. An upstream
//! can still repeat it: a pagination `Link`, a redirect `Location`, an error
//! body quoting the request. [`InjectedSecrets`] collects every form the
//! upstream received, and `proxy::forward_with_redirects` masks them in the
//! reply and in its refusals before either leaves.

use axum::body::{Body, Bytes};
use axum::http::header::{ACCEPT_ENCODING, CONTENT_ENCODING};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;

use crate::api::proxy_pipeline::PipelineOutcome;
use crate::core::{redact_secret_bytes, redact_secret_values};

/// Headers that describe the upstream's exact bytes. A masked body no longer
/// matches them, so they go with the change.
const BYTE_DESCRIBING_HEADERS: [&str; 4] =
    ["content-length", "content-digest", "repr-digest", "digest"];

/// Every form of every credential the pipeline put on the wire, over all
/// hops of one forwarded call.
#[derive(Debug, Default)]
pub(crate) struct InjectedSecrets(Vec<String>);

impl InjectedSecrets {
    /// Record what one hop's pipeline sent: each injected header value and
    /// query value, plus the forms an upstream can derive from them.
    pub(crate) fn add_hop(&mut self, outcome: &PipelineOutcome) {
        let header_values = outcome.headers.iter().map(|(_, value)| value);
        for value in header_values {
            self.add_with_echo_forms(value);
            if let Some((scheme, credentials)) = auth_scheme_credentials(value) {
                self.add_with_echo_forms(credentials);
                if scheme.eq_ignore_ascii_case("basic") {
                    self.add_basic_pair(credentials);
                }
            }
        }
        for (_, value) in &outcome.query {
            self.add_with_echo_forms(value);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The caller's headers for one hop. With a credential on it, the hop asks
    /// for an uncompressed reply, which is the only kind the proxy can mask.
    pub(crate) fn request_headers(&self, caller: &HeaderMap) -> HeaderMap {
        let mut headers = caller.clone();
        if !self.is_empty() {
            headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        }
        headers
    }

    pub(crate) fn redact_text(&self, text: &str) -> String {
        redact_secret_values(text, &self.0)
    }

    /// The reply with every injected credential masked in its headers and
    /// body. A reply that names none comes back byte for byte.
    ///
    /// An encoded body cannot be read, so it is refused rather than passed on
    /// unchecked. The hop asked for `identity`, so only an upstream that
    /// ignored the request sends one.
    pub(crate) async fn redact_response(
        &self,
        response: Response,
    ) -> Result<Response, (StatusCode, String)> {
        if self.is_empty() {
            return Ok(response);
        }
        let (mut parts, body) = response.into_parts();
        // `forward_request` buffers every reply whole, so this reads memory.
        let body = axum::body::to_bytes(body, usize::MAX).await.map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("could not read the reply: {e}"),
            )
        })?;
        if let Some(encoding) = unreadable_encoding(&parts.headers, &body) {
            // The coding name is upstream text, so it can carry an echo too.
            return Err((
                StatusCode::BAD_GATEWAY,
                self.redact_text(&format!(
                    "upstream sent a '{encoding}'-encoded body after the proxy asked for none, \
                     so the proxy cannot check it for the injected credential"
                )),
            ));
        }
        for value in parts.headers.values_mut() {
            if let Some(masked) = redact_secret_bytes(value.as_bytes(), &self.0) {
                // The marker is visible ASCII, so a valid value stays valid.
                *value = HeaderValue::from_bytes(&masked)
                    .unwrap_or_else(|_| HeaderValue::from_static(crate::core::REDACTED_MARKER));
            }
        }
        let body = match redact_secret_bytes(&body, &self.0) {
            None => body,
            Some(masked) => {
                for header in BYTE_DESCRIBING_HEADERS {
                    parts.headers.remove(HeaderName::from_static(header));
                }
                Bytes::from(masked)
            }
        };
        Ok(Response::from_parts(parts, Body::from(body)))
    }

    fn add(&mut self, form: String) {
        if !self.0.contains(&form) {
            self.0.push(form);
        }
    }

    /// `value` and the forms an echo may carry it in. A URL percent-encodes
    /// it, in either hex case, with a space as `%20` or `+`. A JSON encoder
    /// may write `/` as `\/`.
    fn add_with_echo_forms(&mut self, value: &str) {
        let encoded = urlencoding::encode(value).into_owned();
        let lower_hex = lowercase_percent_escapes(&encoded);
        if value.contains('/') {
            self.add(value.replace('/', "\\/"));
        }
        if value.contains(' ') {
            self.add(encoded.replace("%20", "+"));
            self.add(lower_hex.replace("%20", "+"));
        }
        self.add(value.to_string());
        self.add(encoded);
        self.add(lower_hex);
    }

    /// The `user:password` pair a Basic credential decodes to, and the
    /// password alone. An upstream that decoded the header can quote either.
    fn add_basic_pair(&mut self, credentials: &str) {
        use base64::Engine;
        let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(credentials) else {
            return;
        };
        let Ok(pair) = String::from_utf8(decoded) else {
            return;
        };
        if let Some((_, password)) = pair.split_once(':') {
            self.add_with_echo_forms(password);
        }
        self.add_with_echo_forms(&pair);
    }
}

/// Split an `Authorization`-style value into its scheme and its single
/// credential token (RFC 9110 §11.4), as in `Bearer <token>`. A value with
/// parameters after the scheme yields `None`: it has no one token to split.
fn auth_scheme_credentials(value: &str) -> Option<(&str, &str)> {
    let (scheme, credentials) = value.split_once(' ')?;
    let credentials = credentials.trim_start_matches(' ');
    let is_token_char = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    let scheme_ok = !scheme.is_empty() && scheme.chars().all(is_token_char);
    let credentials_ok =
        !credentials.is_empty() && !credentials.contains(|c: char| c.is_whitespace() || c == ',');
    (scheme_ok && credentials_ok).then_some((scheme, credentials))
}

/// `%2B` read as `%2b`. Percent escapes are case-insensitive, so an upstream
/// may echo either.
fn lowercase_percent_escapes(encoded: &str) -> String {
    let mut out = String::with_capacity(encoded.len());
    let mut chars = encoded.chars();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '%' {
            out.extend(chars.by_ref().take(2).map(|h| h.to_ascii_lowercase()));
        }
    }
    out
}

/// The body's content coding, unless it is one the proxy can read as is.
fn unreadable_encoding(headers: &HeaderMap, body: &Bytes) -> Option<String> {
    if body.is_empty() {
        return None;
    }
    headers
        .get_all(CONTENT_ENCODING)
        .iter()
        .map(|value| String::from_utf8_lossy(value.as_bytes()).trim().to_string())
        .find(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case("identity"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(headers: &[(&str, &str)], query: &[(&str, &str)]) -> PipelineOutcome {
        PipelineOutcome {
            headers: headers
                .iter()
                .map(|(n, v)| (HeaderName::from_bytes(n.as_bytes()).unwrap(), v.to_string()))
                .collect(),
            query: query
                .iter()
                .map(|(n, v)| (n.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    fn secrets_of(outcome: &PipelineOutcome) -> Vec<String> {
        let mut secrets = InjectedSecrets::default();
        secrets.add_hop(outcome);
        secrets.0
    }

    #[test]
    fn a_bearer_header_yields_the_whole_value_and_its_token() {
        let secrets = secrets_of(&outcome(&[("authorization", "Bearer tok-abcd")], &[]));
        assert!(secrets.contains(&"Bearer tok-abcd".to_string()));
        assert!(secrets.contains(&"tok-abcd".to_string()));
        assert!(
            !secrets.contains(&"Bearer".to_string()),
            "the scheme alone is no secret: {secrets:?}"
        );
    }

    #[test]
    fn a_basic_header_yields_its_decoded_pair_and_password() {
        // base64("user:hunter22")
        let secrets = secrets_of(&outcome(
            &[("authorization", "Basic dXNlcjpodW50ZXIyMg==")],
            &[],
        ));
        for form in ["dXNlcjpodW50ZXIyMg==", "user:hunter22", "hunter22"] {
            assert!(secrets.contains(&form.to_string()), "{form}: {secrets:?}");
        }
        assert!(
            !secrets.contains(&"user".to_string()),
            "the user name is no secret: {secrets:?}"
        );
    }

    #[test]
    fn a_parameterised_auth_value_yields_only_the_whole_value() {
        let value = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/scope, Signature=abcd";
        let secrets = secrets_of(&outcome(&[("authorization", value)], &[]));
        assert!(secrets.contains(&value.to_string()));
        assert!(
            !secrets.iter().any(|s| s.starts_with("Credential=")),
            "{secrets:?}"
        );
    }

    #[test]
    fn a_query_value_yields_its_percent_encoded_forms_in_both_cases() {
        let secrets = secrets_of(&outcome(&[], &[("api_key", "k+e/y=")]));
        for form in ["k+e/y=", "k%2Be%2Fy%3D", "k%2be%2fy%3d"] {
            assert!(secrets.contains(&form.to_string()), "{form}: {secrets:?}");
        }
    }

    #[test]
    fn a_value_with_a_space_yields_its_form_encoded_forms() {
        let secrets = secrets_of(&outcome(&[], &[("token", "ab cd/e")]));
        for form in ["ab+cd%2Fe", "ab+cd%2fe"] {
            assert!(secrets.contains(&form.to_string()), "{form}: {secrets:?}");
        }
    }

    #[test]
    fn a_value_with_a_slash_yields_its_json_escaped_form() {
        let secrets = secrets_of(&outcome(&[("x-api-key", "ab/cd/ef")], &[]));
        assert!(secrets.contains(&"ab\\/cd\\/ef".to_string()), "{secrets:?}");
    }

    #[tokio::test]
    async fn the_encoded_body_refusal_masks_an_echo_in_the_coding_name() {
        let mut secrets = InjectedSecrets::default();
        secrets.add_hop(&outcome(&[("x-api-key", "sk-live-1234")], &[]));
        let response = Response::builder()
            .header(CONTENT_ENCODING, "sk-live-1234")
            .body(Body::from("opaque"))
            .unwrap();
        let (status, message) = secrets.redact_response(response).await.unwrap_err();
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert!(!message.contains("sk-live-1234"), "{message}");
        assert!(message.contains(crate::core::REDACTED_MARKER), "{message}");
    }

    #[test]
    fn a_hop_with_no_credential_leaves_the_caller_headers_alone() {
        let mut caller = HeaderMap::new();
        caller.insert(ACCEPT_ENCODING, HeaderValue::from_static("gzip"));
        let secrets = InjectedSecrets::default();
        assert_eq!(secrets.request_headers(&caller), caller);
    }

    #[test]
    fn an_identity_or_absent_coding_is_readable() {
        let body = Bytes::from_static(b"x");
        let mut headers = HeaderMap::new();
        assert_eq!(unreadable_encoding(&headers, &body), None);
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("identity"));
        assert_eq!(unreadable_encoding(&headers, &body), None);
        headers.insert(CONTENT_ENCODING, HeaderValue::from_static("br"));
        assert_eq!(unreadable_encoding(&headers, &body), Some("br".to_string()));
        assert_eq!(unreadable_encoding(&headers, &Bytes::new()), None);
    }
}
