//! A Claude gateway on loopback, for tests: discovery, device authorization, a
//! token endpoint answering from a script, revocation, managed settings, an
//! OTLP logs receiver, and a paged model listing.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::extract::{Form, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Json;

use super::discovery::tests::document;
use super::GatewayOrigin;

/// One request the mock saw.
#[derive(Debug, Clone)]
pub(crate) struct Seen {
    pub path: String,
    pub form: HashMap<String, String>,
    pub user_agent: Option<String>,
    pub authorization: Option<String>,
    pub if_none_match: Option<String>,
}

/// What `GET /managed/settings` answers: a status, a body, and an `ETag`. A
/// request whose `If-None-Match` names the `ETag` gets `304`.
#[derive(Clone)]
struct ManagedSettingsReply {
    status: StatusCode,
    body: serde_json::Value,
    etag: Option<String>,
}

type Replies = Arc<Mutex<VecDeque<(StatusCode, serde_json::Value)>>>;

/// One OTLP export the mock received at `POST /v1/logs`.
#[derive(Debug, Clone)]
pub(crate) struct LogsRequest {
    pub body: Vec<u8>,
    pub headers: HeaderMap,
}

impl LogsRequest {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

/// The model listing: every id, and how many one page holds.
struct Listing {
    ids: Vec<String>,
    page_size: usize,
    /// A bearer the listing answers `401` to.
    rejected_bearer: Option<String>,
}

pub(crate) struct MockGateway {
    pub origin: GatewayOrigin,
    seen: Arc<Mutex<Vec<Seen>>>,
    token_replies: Replies,
    device_reply: Arc<Mutex<serde_json::Value>>,
    listing: Arc<Mutex<Listing>>,
    managed_settings: Arc<Mutex<ManagedSettingsReply>>,
    logs: Arc<Mutex<Vec<LogsRequest>>>,
    logs_replies: Arc<Mutex<VecDeque<StatusCode>>>,
}

impl MockGateway {
    pub(crate) async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let token_replies: Replies = Arc::default();
        let device_reply = Arc::new(Mutex::new(serde_json::json!({
            "device_code": "device-code",
            "user_code": "ABCD-EFGH",
            "verification_uri": format!("{origin}/device"),
            "verification_uri_complete": format!("{origin}/device?user_code=ABCD-EFGH"),
            "expires_in": 600,
            "interval": 0,
        })));

        let listing = Arc::new(Mutex::new(Listing {
            ids: Vec::new(),
            page_size: 1000,
            rejected_bearer: None,
        }));

        let record = |seen: &Arc<Mutex<Vec<Seen>>>, path: &str, headers: &HeaderMap, form| {
            let header = |name: &str| {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            };
            seen.lock().unwrap().push(Seen {
                path: path.to_string(),
                form,
                user_agent: header("user-agent"),
                authorization: header("authorization"),
                if_none_match: header("if-none-match"),
            });
        };
        let discovery = {
            let (seen, doc) = (seen.clone(), document(&origin));
            move |headers: HeaderMap| async move {
                record(
                    &seen,
                    "/.well-known/oauth-authorization-server",
                    &headers,
                    HashMap::new(),
                );
                Json(doc)
            }
        };
        let device = {
            let (seen, reply) = (seen.clone(), device_reply.clone());
            move |headers: HeaderMap, Form(form): Form<HashMap<String, String>>| async move {
                record(&seen, "/oauth/device_authorization", &headers, form);
                let body = reply.lock().unwrap().clone();
                Json(body)
            }
        };
        let token = {
            let (seen, replies) = (seen.clone(), token_replies.clone());
            move |headers: HeaderMap, Form(form): Form<HashMap<String, String>>| async move {
                record(&seen, "/oauth/token", &headers, form);
                let reply = replies.lock().unwrap().pop_front();
                let (status, body) = reply.unwrap_or((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"error": "unscripted"}),
                ));
                (status, Json(body))
            }
        };
        let revoke = {
            let seen = seen.clone();
            move |headers: HeaderMap, Form(form): Form<HashMap<String, String>>| async move {
                record(&seen, "/oauth/revoke", &headers, form);
                StatusCode::OK
            }
        };
        let managed_settings = Arc::new(Mutex::new(ManagedSettingsReply {
            status: StatusCode::NOT_FOUND,
            body: serde_json::json!({}),
            etag: None,
        }));
        let managed = {
            let (seen, reply) = (seen.clone(), managed_settings.clone());
            move |headers: HeaderMap| async move {
                record(&seen, "/managed/settings", &headers, HashMap::new());
                let reply = reply.lock().unwrap().clone();
                let if_none_match = headers.get("if-none-match").and_then(|v| v.to_str().ok());
                if reply.etag.is_some() && reply.etag.as_deref() == if_none_match {
                    return StatusCode::NOT_MODIFIED.into_response();
                }
                let mut response = (reply.status, Json(reply.body)).into_response();
                if let Some(etag) = reply.etag {
                    response.headers_mut().insert("etag", etag.parse().unwrap());
                }
                response
            }
        };
        let logs: Arc<Mutex<Vec<LogsRequest>>> = Arc::default();
        let logs_replies: Arc<Mutex<VecDeque<StatusCode>>> = Arc::default();
        let export = {
            let (received, replies) = (logs.clone(), logs_replies.clone());
            move |headers: HeaderMap, body: axum::body::Bytes| async move {
                received.lock().unwrap().push(LogsRequest {
                    body: body.to_vec(),
                    headers,
                });
                replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(StatusCode::OK)
            }
        };
        let models = {
            let (seen, listing) = (seen.clone(), listing.clone());
            move |headers: HeaderMap, Query(query): Query<HashMap<String, String>>| async move {
                record(&seen, "/v1/models", &headers, query.clone());
                let listing = listing.lock().unwrap();
                let bearer = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "));
                if bearer.is_some() && bearer == listing.rejected_bearer.as_deref() {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                let start = query
                    .get("after_id")
                    .and_then(|after| listing.ids.iter().position(|id| id == after))
                    .map_or(0, |at| at + 1);
                let page: Vec<&String> = listing
                    .ids
                    .iter()
                    .skip(start)
                    .take(listing.page_size)
                    .collect();
                Json(serde_json::json!({
                    "data": page.iter().map(|id| serde_json::json!({"type": "model", "id": id})).collect::<Vec<_>>(),
                    "has_more": start + page.len() < listing.ids.len(),
                    "first_id": page.first(),
                    "last_id": page.last(),
                }))
                .into_response()
            }
        };
        let app = axum::Router::new()
            .route("/.well-known/oauth-authorization-server", get(discovery))
            .route("/oauth/device_authorization", post(device))
            .route("/oauth/token", post(token))
            .route("/oauth/revoke", post(revoke))
            .route("/managed/settings", get(managed))
            .route("/v1/logs", post(export))
            .route("/v1/models", get(models));
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        Self {
            origin: GatewayOrigin::parse(&origin).unwrap(),
            seen,
            token_replies,
            device_reply,
            listing,
            managed_settings,
            logs,
            logs_replies,
        }
    }

    /// The OTLP exports received, oldest first.
    pub(crate) fn logs_requests(&self) -> Vec<LogsRequest> {
        self.logs.lock().unwrap().clone()
    }

    /// Queue the status of the next OTLP export. Unqueued, an export gets `200`.
    pub(crate) fn reply_to_logs(&self, status: StatusCode) {
        self.logs_replies.lock().unwrap().push_back(status);
    }

    /// Answer `GET /managed/settings` with `status` and `body`, tagged `etag`.
    /// Until called, the gateway has no policy (`404`).
    pub(crate) fn serve_managed_settings(
        &self,
        status: StatusCode,
        body: serde_json::Value,
        etag: Option<&str>,
    ) {
        *self.managed_settings.lock().unwrap() = ManagedSettingsReply {
            status,
            body,
            etag: etag.map(str::to_string),
        };
    }

    /// Serve `ids` from `GET /v1/models`, `page_size` to a page.
    pub(crate) fn list_models(&self, ids: &[&str], page_size: usize) {
        let mut listing = self.listing.lock().unwrap();
        listing.ids = ids.iter().map(|id| id.to_string()).collect();
        listing.page_size = page_size;
    }

    /// Answer `401` to a listing read with `bearer`.
    pub(crate) fn reject_listing_for(&self, bearer: &str) {
        self.listing.lock().unwrap().rejected_bearer = Some(bearer.to_string());
    }

    /// Queue the token endpoint's next answer.
    pub(crate) fn reply_to_token(&self, status: StatusCode, body: serde_json::Value) {
        self.token_replies.lock().unwrap().push_back((status, body));
    }

    /// Queue an OAuth error answer, such as `authorization_pending`.
    pub(crate) fn refuse_token(&self, error: &str) {
        self.reply_to_token(
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "error": error }),
        );
    }

    /// Change one field of the device authorization answer.
    pub(crate) fn set_device_field(&self, field: &str, value: serde_json::Value) {
        self.device_reply.lock().unwrap()[field] = value;
    }

    pub(crate) fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    pub(crate) fn paths(&self) -> Vec<String> {
        self.seen().into_iter().map(|s| s.path).collect()
    }

    /// The forms posted to the token endpoint, oldest first.
    pub(crate) fn token_forms(&self) -> Vec<HashMap<String, String>> {
        self.seen()
            .into_iter()
            .filter(|s| s.path == "/oauth/token")
            .map(|s| s.form)
            .collect()
    }
}
