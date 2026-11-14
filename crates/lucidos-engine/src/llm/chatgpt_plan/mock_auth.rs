//! OpenAI's auth host on loopback, for tests: discovery, the key set, a token
//! endpoint answering from a script, and revocation.
//!
//! Like OpenAI, it refuses a refresh token it already rotated away with
//! `refresh_token_reused`.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex};

use axum::extract::Form;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Json;

use super::auth_server::AuthServer;
use super::id_token::tests::test_jwks;

type Replies = Arc<Mutex<VecDeque<(StatusCode, serde_json::Value)>>>;

/// Every form posted, with the endpoint it went to.
type Forms = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;

pub(crate) struct MockAuth {
    pub auth: AuthServer,
    forms: Forms,
    token_replies: Replies,
}

impl MockAuth {
    pub(crate) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let forms: Forms = Arc::default();
        let token_replies: Replies = Arc::default();
        let spent: Arc<Mutex<HashSet<String>>> = Arc::default();

        let discovery = {
            let origin = origin.clone();
            move || async move {
                Json(serde_json::json!({
                    "issuer": origin,
                    "jwks_uri": format!("{origin}/.well-known/jwks.json"),
                    "id_token_signing_alg_values_supported": ["RS256"],
                }))
            }
        };
        let token = {
            let (forms, replies) = (forms.clone(), token_replies.clone());
            move |Form(form): Form<HashMap<String, String>>| async move {
                forms.lock().unwrap().push(("token".into(), form.clone()));
                if form.get("grant_type").map(String::as_str) == Some("refresh_token") {
                    let presented = form.get("refresh_token").cloned().unwrap_or_default();
                    if !spent.lock().unwrap().insert(presented) {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(serde_json::json!({"error": "refresh_token_reused"})),
                        );
                    }
                }
                let reply = replies.lock().unwrap().pop_front();
                let (status, body) = reply.unwrap_or((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": "unscripted"}),
                ));
                (status, Json(body))
            }
        };
        let revoke = {
            let forms = forms.clone();
            move |Form(form): Form<HashMap<String, String>>| async move {
                forms.lock().unwrap().push(("revoke".into(), form));
                StatusCode::OK
            }
        };
        let app = axum::Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route(
                "/.well-known/jwks.json",
                get(|| async { Json(test_jwks()) }),
            )
            .route("/api/accounts/oauth/token", post(token))
            .route("/api/accounts/oauth/revoke", post(revoke));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        Self {
            auth: AuthServer::parse(&origin).unwrap(),
            forms,
            token_replies,
        }
    }

    /// Queue the token endpoint's next answer.
    pub(crate) fn reply_to_token(&self, status: StatusCode, body: serde_json::Value) {
        self.token_replies.lock().unwrap().push_back((status, body));
    }

    /// The forms posted to the token endpoint, oldest first.
    pub(crate) fn token_forms(&self) -> Vec<HashMap<String, String>> {
        self.forms_to("token")
    }

    /// The forms posted to the revocation endpoint, oldest first.
    pub(crate) fn revoke_forms(&self) -> Vec<HashMap<String, String>> {
        self.forms_to("revoke")
    }

    fn forms_to(&self, endpoint: &str) -> Vec<HashMap<String, String>> {
        self.forms
            .lock()
            .unwrap()
            .iter()
            .filter(|(to, _)| to == endpoint)
            .map(|(_, form)| form.clone())
            .collect()
    }
}
