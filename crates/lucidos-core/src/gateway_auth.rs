use lucidos_local_token::{read as local_token, HEADER_LOCAL_TOKEN};
use reqwest::header::{HeaderMap, HeaderValue};

pub fn auth_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(token) = local_token() {
        if let Ok(mut value) = HeaderValue::from_str(&token) {
            value.set_sensitive(true);
            headers.insert(HEADER_LOCAL_TOKEN, value);
        }
    }
    headers
}

pub fn client_builder() -> reqwest::ClientBuilder {
    lucidos_local_token::client::client_builder().default_headers(auth_headers())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_credential_is_marked_sensitive_so_it_stays_out_of_debug_output() {
        let mut headers = HeaderMap::new();
        let mut value = HeaderValue::from_static("secret-token");
        value.set_sensitive(true);
        headers.insert(HEADER_LOCAL_TOKEN, value);
        assert!(!format!("{headers:?}").contains("secret-token"));
    }

    #[test]
    fn no_token_yields_no_header_rather_than_an_empty_one() {
        let headers = auth_headers();
        if let Some(value) = headers.get(HEADER_LOCAL_TOKEN) {
            assert!(!value.is_empty(), "an empty credential must not be sent");
        }
    }
}
