use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::value::RawValue;
use serde_json::{json, Value};
use tower::ServiceExt;

use super::*;
use crate::api::gateway_stamp::test_support::stamped_device_headers;
use crate::api::permission_requests::tests::permission_test_state;
use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};

const PHONE: &str = "owner-phone";
const LAPTOP: &str = "owner-laptop";

const SLICE_ROUTES: [(&str, &str); 25] = [
    ("preferences", "/preferences?device_id={device}"),
    (
        "unread_notifications",
        "/notifications?limit=100&filter=unread",
    ),
    ("reader_devices", "/users/me"),
    ("users", "/users"),
    ("pinned_apps", "/pinned-apps?device_id={device}"),
    ("apps", "/apps"),
    ("chat_models", "/models"),
    ("intelligence_ladder", "/intelligence-ladder?surface=chat"),
    ("spend_cap_holds", "/models/spend-cap-holds"),
    ("last_successful_backup", "/backup/last-successful"),
    ("release_notices", "/release-notices"),
    ("embedding_model_status", "/memory/embedding-model-status"),
    ("webhook_ingress", "/webhooks/ingress"),
    ("webhook_refusals", "/webhooks/refusals"),
    ("installed_plugins", "/plugins/installed"),
    ("plugin_catalog", "/plugins/catalog"),
    ("triggers", "/triggers"),
    ("historical_triggers", "/triggers/historical"),
    ("trigger_groups", "/trigger-groups"),
    ("thread_queue", "/thread-queue"),
    ("repositories", "/repositories"),
    ("devices", "/devices"),
    ("filter_facets", "/threads/filter-facets"),
    ("changes_state", "/changes?limit=15"),
    ("engine_version_status", "/engine/version-status"),
];

const SNAPSHOT_BUDGET_BYTES: usize = 512 * 1024;

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

async fn send(state: &AppState, request: Request<Body>) -> Answer {
    let response = crate::api::api_routes(state)
        .oneshot(request)
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    Answer {
        status,
        headers,
        body: String::from_utf8(bytes.to_vec()).unwrap(),
    }
}

async fn get_as(state: &AppState, uri: &str, headers: HeaderMap) -> Answer {
    let mut request = Request::builder().uri(uri).body(Body::empty()).unwrap();
    request.headers_mut().extend(headers);
    send(state, request).await
}

async fn bootstrap_as(state: &AppState, device: &str) -> Answer {
    get_as(state, "/bootstrap", stamped_device_headers(device)).await
}

async fn post_as(state: &AppState, device: &str, uri: &str, body: Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.headers_mut().extend(stamped_device_headers(device));
    let answer = send(state, request).await;
    assert_eq!(answer.status, StatusCode::OK, "{uri}: {}", answer.body);
}

fn slices_of(snapshot: &Answer) -> Vec<(String, String)> {
    assert_eq!(snapshot.status, StatusCode::OK, "{}", snapshot.body);
    let keys = served_keys(&snapshot.body);
    let by_key: BTreeMap<String, Box<RawValue>> = serde_json::from_str(&snapshot.body).unwrap();
    keys.into_iter()
        .map(|key| {
            let slice = by_key[&key].get().to_owned();
            (key, slice)
        })
        .collect()
}

fn served_keys(body: &str) -> Vec<String> {
    struct Keys(Vec<String>);
    impl<'de> serde::Deserialize<'de> for Keys {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = Keys;
                fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    f.write_str("an object")
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Keys, A::Error> {
                    let mut keys = Vec::new();
                    while let Some((key, _)) = map.next_entry::<String, serde::de::IgnoredAny>()? {
                        keys.push(key);
                    }
                    Ok(Keys(keys))
                }
            }
            deserializer.deserialize_map(Visitor)
        }
    }
    serde_json::from_str::<Keys>(body).unwrap().0
}

fn slice_of_route(route: &Answer) -> String {
    if route.status.is_success() {
        return format!(r#"{{"data":{}}}"#, route.body);
    }
    let message = match serde_json::from_str::<Value>(&route.body) {
        Ok(body) => body["error"].as_str().unwrap().to_owned(),
        Err(_) => route.body.trim().to_owned(),
    };
    assert!(!message.is_empty(), "a failing route says why");
    format!(
        r#"{{"error":{{"message":{},"http_code":{}}}}}"#,
        Value::from(message),
        route.status.as_u16()
    )
}

async fn assert_every_slice_is_its_route(state: &AppState, device: &str) -> Vec<(String, String)> {
    let slices = slices_of(&bootstrap_as(state, device).await);
    let served: Vec<&str> = slices.iter().map(|(key, _)| key.as_str()).collect();
    let planned: Vec<&str> = SLICE_ROUTES.iter().map(|(key, _)| *key).collect();
    assert_eq!(served, planned, "every key, on every 200");
    for ((key, slice), (_, uri)) in slices.iter().zip(SLICE_ROUTES) {
        let uri = uri.replace("{device}", device);
        let route = get_as(state, &uri, stamped_device_headers(device)).await;
        assert_eq!(slice, &slice_of_route(&route), "{key} against {uri}");
    }
    slices
}

fn slice<'a>(slices: &'a [(String, String)], key: &str) -> &'a str {
    &slices.iter().find(|(served, _)| served == key).unwrap().1
}

fn failed_slices(slices: &[(String, String)]) -> Vec<&str> {
    slices
        .iter()
        .filter(|(_, slice)| !slice.starts_with(r#"{"data":"#))
        .map(|(key, _)| key.as_str())
        .collect()
}

const FAILS_IN_A_TEST_BUILD: &str = "release_notices";

async fn first_catalog_scan_lands(state: &AppState) {
    for _ in 0..200 {
        let catalog = get_as(state, "/plugins/catalog", HeaderMap::new()).await;
        if serde_json::from_str::<Value>(&catalog.body).unwrap()["scanning"] == false {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("the first catalog scan never landed");
}

async fn pin_app(state: &AppState, device: &str, app_id: &str) {
    crate::core::PinnedAppStore::pin(
        &state.pool,
        &state.engine.event_bus,
        app_id,
        "main",
        device,
        None,
    )
    .await
    .unwrap();
}

async fn notify(state: &AppState, title: &str) {
    let body = json!({ "title": title, "message": "A message long enough to count." });
    post_as(state, PHONE, "/notifications", body).await;
}

#[test]
fn the_table_is_split_by_where_each_handler_goes() {
    assert_eq!(API_SLICES.len(), 22);
    let threads_api: Vec<&str> = THREADS_API_SLICES.iter().map(|row| row.key).collect();
    assert_eq!(
        threads_api,
        ["filter_facets", "changes_state", "engine_version_status"]
    );
    let served: Vec<&str> = slice_readers().map(|row| row.key).collect();
    let planned: Vec<&str> = SLICE_ROUTES.iter().map(|(key, _)| *key).collect();
    assert_eq!(served, planned);
}

#[tokio::test]
async fn a_refusal_keeps_the_code_and_message_its_route_sends() {
    use crate::api::error::ApiError;
    let as_json = |slice: SliceResult| serde_json::to_value(slice).unwrap();

    let json_error = SliceResult::of(Err::<(), _>(ApiError::not_found("No such thing"))).await;
    assert_eq!(
        as_json(json_error),
        json!({ "error": { "message": "No such thing", "http_code": 404 } })
    );
    let text_error = SliceResult::of(Err::<(), _>((
        StatusCode::BAD_GATEWAY,
        "Upstream is down".to_owned(),
    )))
    .await;
    assert_eq!(
        as_json(text_error),
        json!({ "error": { "message": "Upstream is down", "http_code": 502 } })
    );
    let bare = SliceResult::of(Err::<(), _>(StatusCode::CONFLICT)).await;
    assert_eq!(
        as_json(bare),
        json!({ "error": { "message": "Conflict", "http_code": 409 } })
    );
    let data = SliceResult::of(Ok::<_, StatusCode>(json!({ "n": 1.5 }))).await;
    assert_eq!(
        serde_json::to_string(&data).unwrap(),
        r#"{"data":{"n":1.5}}"#
    );
}

#[tokio::test]
async fn every_slice_is_the_body_its_own_route_answers() {
    if !in_engine_child("api::bootstrap::tests::every_slice_is_the_body_its_own_route_answers")
        .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    pin_app(&state, PHONE, "habit-tracker").await;
    notify(&state, "Backup finished").await;
    crate::core::PreferenceStore::set_for_device(
        &state.pool,
        &state.engine.event_bus,
        "theme",
        "dark",
        PHONE,
        None,
    )
    .await
    .unwrap();
    first_catalog_scan_lands(&state).await;

    let slices = assert_every_slice_is_its_route(&state, PHONE).await;
    assert_eq!(failed_slices(&slices), [FAILS_IN_A_TEST_BUILD]);
    assert!(slice(&slices, "pinned_apps").contains("habit-tracker"));
    assert!(slice(&slices, "unread_notifications").contains("Backup finished"));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_failing_source_fails_only_its_own_slice() {
    if !in_engine_child("api::bootstrap::tests::a_failing_source_fails_only_its_own_slice").await {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let registry = workspace
        .path()
        .join(crate::core::DATA_DIR)
        .join(crate::core::plugin_marketplaces::MARKETPLACES_DATA_PATH);
    std::fs::create_dir_all(registry.parent().unwrap()).unwrap();
    std::fs::write(&registry, "not a registry").unwrap();
    sqlx::query("ALTER TABLE pinned_apps RENAME TO pinned_apps_gone")
        .execute(&state.pool)
        .await
        .unwrap();

    let slices = assert_every_slice_is_its_route(&state, PHONE).await;
    assert_eq!(
        failed_slices(&slices),
        ["pinned_apps", FAILS_IN_A_TEST_BUILD, "plugin_catalog"]
    );
    assert_eq!(slices.len(), SLICE_ROUTES.len(), "a failure drops no key");
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_caller_naming_no_device_is_refused_the_way_the_route_refuses_it() {
    if !in_engine_child(
        "api::bootstrap::tests::a_caller_naming_no_device_is_refused_the_way_the_route_refuses_it",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;

    let slices = slices_of(&get_as(&state, "/bootstrap", HeaderMap::new()).await);
    assert_eq!(slices.len(), SLICE_ROUTES.len());
    let route = get_as(&state, "/pinned-apps", HeaderMap::new()).await;
    assert_eq!(route.status, StatusCode::BAD_REQUEST);
    assert_eq!(slice(&slices, "pinned_apps"), slice_of_route(&route));
    let shared = get_as(&state, "/preferences", HeaderMap::new()).await;
    assert_eq!(slice(&slices, "preferences"), slice_of_route(&shared));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn each_device_reads_its_own_view_and_no_shared_cache_may_keep_it() {
    if !in_engine_child(
        "api::bootstrap::tests::each_device_reads_its_own_view_and_no_shared_cache_may_keep_it",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    pin_app(&state, PHONE, "habit-tracker").await;

    let phone = bootstrap_as(&state, PHONE).await;
    let laptop = bootstrap_as(&state, LAPTOP).await;
    let (phone_slices, laptop_slices) = (slices_of(&phone), slices_of(&laptop));
    assert!(slice(&phone_slices, "pinned_apps").contains("habit-tracker"));
    assert!(!slice(&laptop_slices, "pinned_apps").contains("habit-tracker"));
    assert_ne!(phone.headers[header::ETAG], laptop.headers[header::ETAG]);
    for answer in [&phone, &laptop] {
        assert_eq!(answer.headers[header::CACHE_CONTROL], "private, no-cache");
        assert_eq!(answer.headers[header::VARY], HEADER_DEVICE_ID);
        assert_eq!(answer.headers[header::CONTENT_TYPE], "application/json");
    }
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unchanged_workspace_answers_not_modified() {
    if !in_engine_child("api::bootstrap::tests::an_unchanged_workspace_answers_not_modified").await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;

    first_catalog_scan_lands(&state).await;

    let first = bootstrap_as(&state, PHONE).await;
    let second = bootstrap_as(&state, PHONE).await;
    let etag = first.headers[header::ETAG].clone();
    assert!(etag.to_str().unwrap().starts_with("W/\""), "{etag:?}");
    assert_eq!(
        etag,
        second.headers[header::ETAG],
        "{}",
        first_difference(&first, &second)
    );

    let mut held = stamped_device_headers(PHONE);
    held.insert(header::IF_NONE_MATCH, etag.clone());
    let unchanged = get_as(&state, "/bootstrap", held.clone()).await;
    assert_eq!(unchanged.status, StatusCode::NOT_MODIFIED);
    assert!(unchanged.body.is_empty());
    assert_eq!(unchanged.headers[header::ETAG], etag);
    assert_eq!(
        unchanged.headers[header::CACHE_CONTROL],
        "private, no-cache"
    );

    pin_app(&state, PHONE, "habit-tracker").await;
    let changed = get_as(&state, "/bootstrap", held).await;
    assert_eq!(changed.status, StatusCode::OK);
    assert_ne!(changed.headers[header::ETAG], etag);
    teardown_test_db(&db).await;
}

fn first_difference(first: &Answer, second: &Answer) -> String {
    slices_of(first)
        .into_iter()
        .zip(slices_of(second))
        .find(|(a, b)| a != b)
        .map(|((key, a), (_, b))| format!("{key} moved:\n{a}\n{b}"))
        .unwrap_or_default()
}

#[tokio::test]
async fn the_snapshot_stays_inside_its_size_budget() {
    if !in_engine_child("api::bootstrap::tests::the_snapshot_stays_inside_its_size_budget").await {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    let past_page = notifications::NOTIFICATIONS_PAGE_MAX + 20;
    for n in 0..past_page {
        notify(&state, &format!("Notification {n}")).await;
    }
    let past_default = changes::CHANGES_DEFAULT_LIMIT + 10;
    for n in 0..past_default {
        sqlx::query(
            "INSERT INTO changes (id, request_id, branch_name, repo_root, status, resolved_at) \
             VALUES ($1, $2, $3, '/repo', 'applied', now())",
        )
        .bind(uuid::Uuid::new_v4())
        .bind(uuid::Uuid::new_v4())
        .bind(format!("branch-{n}"))
        .execute(&state.pool)
        .await
        .unwrap();
    }

    let snapshot = bootstrap_as(&state, PHONE).await;
    assert!(
        snapshot.body.len() < SNAPSHOT_BUDGET_BYTES,
        "{} bytes",
        snapshot.body.len()
    );
    let body: Value = serde_json::from_str(&snapshot.body).unwrap();
    let unread = &body["unread_notifications"]["data"];
    assert_eq!(
        unread["notifications"].as_array().unwrap().len() as i64,
        notifications::NOTIFICATIONS_PAGE_MAX
    );
    assert_eq!(unread["has_more"], true);
    let changes_state = &body["changes_state"]["data"];
    assert_eq!(
        changes_state["applied"].as_array().unwrap().len() as i64,
        changes::CHANGES_DEFAULT_LIMIT
    );
    assert_eq!(changes_state["has_more_applied"], true);
    teardown_test_db(&db).await;
}

#[test]
fn a_validator_on_any_if_none_match_line_answers_not_modified() {
    let body = br#"{"preferences":{"data":{}}}"#.to_vec();
    let full = snapshot_response(body.clone(), &HeaderMap::new());
    assert_eq!(full.status(), StatusCode::OK);
    let etag = full.headers()[header::ETAG].clone();

    let mut held = HeaderMap::new();
    held.append(header::IF_NONE_MATCH, "W/\"stale\"".parse().unwrap());
    held.append(header::IF_NONE_MATCH, etag);
    let unchanged = snapshot_response(body, &held);
    assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
}

#[test]
fn both_answers_vary_on_the_device_and_the_encoding() {
    let body = b"{}".to_vec();
    let full = snapshot_response(body.clone(), &HeaderMap::new());
    let mut held = HeaderMap::new();
    held.insert(header::IF_NONE_MATCH, full.headers()[header::ETAG].clone());
    let unchanged = snapshot_response(body, &held);
    assert_eq!(unchanged.status(), StatusCode::NOT_MODIFIED);
    for answer in [&full, &unchanged] {
        assert!(answer.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .starts_with("W/\""));
        let vary: Vec<&str> = answer
            .headers()
            .get_all(header::VARY)
            .iter()
            .map(|value| value.to_str().unwrap())
            .collect();
        assert_eq!(vary, [HEADER_DEVICE_ID, header::ACCEPT_ENCODING.as_str()]);
    }
}

#[test]
fn unchanged_preferences_serialize_to_the_same_bytes() {
    use crate::core::prefs;
    let stored = [
        prefs::THEME_MODE.key(),
        prefs::FONT_FAMILY.key(),
        prefs::CHAT_MODEL.key(),
        prefs::TRIGGER_MODEL.key(),
        prefs::BACKUP_SCHEDULE.key(),
    ];
    let preferences = stored
        .into_iter()
        .map(|key| (key.to_owned(), "set".to_owned()))
        .collect();
    let served = serde_json::to_string(&crate::api::PreferencesResponse { preferences }).unwrap();
    let keys = served_keys(&served[r#"{"preferences":"#.len()..served.len() - 1]);
    let mut in_key_order = stored;
    in_key_order.sort_unstable();
    assert_ne!(stored, in_key_order, "the fixture starts out of order");
    assert_eq!(keys, in_key_order);
}

const PANICKING_READER: SliceReader = SliceReader {
    key: "panicked",
    read: |_| Box::pin(async { panic!("a reader that panics") }),
};

#[tokio::test]
async fn a_reader_that_panics_fails_only_its_own_slice() {
    if !in_engine_child("api::bootstrap::tests::a_reader_that_panics_fails_only_its_own_slice")
        .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;
    static PANICKED: SliceReader = PANICKING_READER;
    static COUNTED: SliceReader = COUNTING_READER;
    let context = SliceContext {
        state,
        headers: HeaderMap::new(),
    };

    let snapshot = read_snapshot(&context, [&COUNTED, &PANICKED, &COUNTED].into_iter()).await;

    let [before, panicked, after] = &snapshot.0[..] else {
        panic!("a panic drops no key: {:?}", snapshot.0);
    };
    assert!(matches!(before, ("counted", SliceResult::Data(_))));
    assert!(matches!(after, ("counted", SliceResult::Data(_))));
    assert_eq!(panicked.0, "panicked");
    assert_eq!(
        serde_json::to_value(&panicked.1).unwrap(),
        json!({ "error": { "message": "Internal Server Error", "http_code": 500 } })
    );
    teardown_test_db(&db).await;
}

static READS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static MOST_READS_IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);

const COUNTING_READER: SliceReader = SliceReader {
    key: "counted",
    read: |_| {
        Box::pin(async {
            let in_flight = READS_IN_FLIGHT.fetch_add(1, Ordering::SeqCst) + 1;
            MOST_READS_IN_FLIGHT.fetch_max(in_flight, Ordering::SeqCst);
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
            READS_IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
            SliceResult::data(&())
        })
    },
};

#[tokio::test]
async fn concurrent_snapshots_leave_the_pool_to_other_requests() {
    if !in_engine_child(
        "api::bootstrap::tests::concurrent_snapshots_leave_the_pool_to_other_requests",
    )
    .await
    {
        return;
    }
    let (_pool, db) = setup_test_db().await;
    let (engine, workspace) = boot_mock_engine(&db).await;
    let state = permission_test_state(engine, workspace.path().to_path_buf()).await;

    static COUNTED: SliceReader = COUNTING_READER;
    let context = SliceContext {
        state: state.clone(),
        headers: HeaderMap::new(),
    };
    let counted = read_snapshot(&context, std::iter::repeat_n(&COUNTED, 25)).await;
    assert_eq!(counted.0.len(), 25);
    assert_eq!(
        MOST_READS_IN_FLIGHT.load(Ordering::SeqCst),
        BOOTSTRAP_READ_CONCURRENCY
    );

    let snapshots = futures::future::join_all((0..6).map(|_| bootstrap_as(&state, PHONE)));
    let pool_read = get_as(&state, "/devices", stamped_device_headers(PHONE));
    let (snapshots, pool_read) = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        futures::future::join(snapshots, pool_read),
    )
    .await
    .expect("no read waits on the pool");
    assert_eq!(pool_read.status, StatusCode::OK, "{}", pool_read.body);
    for snapshot in &snapshots {
        let slices = slices_of(snapshot);
        assert_eq!(slices.len(), SLICE_ROUTES.len());
        assert_eq!(failed_slices(&slices), [FAILS_IN_A_TEST_BUILD]);
    }
    teardown_test_db(&db).await;
}
