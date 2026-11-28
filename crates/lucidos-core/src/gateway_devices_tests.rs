use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};

const MOVE_NAMES: &str =
    include_str!("../migrations/20261127145632_move_device_names_to_the_gateway.sql");

async fn workspace_before_the_move(
    rows: &[(&str, Option<&str>, Option<&str>)],
) -> (PgPool, String) {
    let (pool, db_name) = setup_test_db().await;
    sqlx::raw_sql(
        "ALTER TABLE devices ADD COLUMN name TEXT, ADD COLUMN pairing_label TEXT;
         DELETE FROM device_name_imports;",
    )
    .execute(&pool)
    .await
    .unwrap();
    for (id, name, label) in rows {
        sqlx::query(
            "INSERT INTO devices (id, name, pairing_label, user_agent) VALUES ($1, $2, $3, 'UA')",
        )
        .bind(id)
        .bind(name)
        .bind(label)
        .execute(&pool)
        .await
        .unwrap();
    }
    (pool, db_name)
}

fn held(id: &str, name: Option<&str>, label: Option<&str>) -> HeldDeviceName {
    HeldDeviceName {
        device_id: id.into(),
        name: name.map(str::to_string),
        pairing_label: label.map(str::to_string),
    }
}

#[tokio::test]
async fn the_move_holds_every_name_from_two_workspaces_and_drops_the_columns() {
    let (first, first_db) = workspace_before_the_move(&[
        ("phone", Some("Kitchen iPad"), Some("My iPhone")),
        ("desk", Some("  "), None),
        ("blank", None, None),
    ])
    .await;
    let (second, second_db) = workspace_before_the_move(&[
        ("laptop", None, Some(" Laptop ")),
        ("studio", Some("Studio"), None),
    ])
    .await;

    for pool in [&first, &second] {
        sqlx::raw_sql(MOVE_NAMES).execute(pool).await.unwrap();
    }

    assert_eq!(
        held_names(&first).await.unwrap(),
        vec![held("phone", Some("Kitchen iPad"), Some("My iPhone"))]
    );
    assert_eq!(
        held_names(&second).await.unwrap(),
        vec![
            held("laptop", None, Some("Laptop")),
            held("studio", Some("Studio"), None),
        ]
    );
    let wire = serde_json::to_value(held_names(&second).await.unwrap()).unwrap();
    assert_eq!(
        wire[0],
        serde_json::json!({ "device_id": "laptop", "name": null, "pairing_label": "Laptop" }),
        "the shape the gateway's import route reads"
    );

    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
         WHERE table_name = 'devices' ORDER BY column_name",
    )
    .fetch_all(&first)
    .await
    .unwrap();
    assert!(!columns.iter().any(|c| c == "name" || c == "pairing_label"));
    for kept in ["push_enabled", "last_seen_at", "user_agent"] {
        assert!(columns.iter().any(|c| c == kept), "{kept} stays");
    }

    for (pool, db) in [(first, first_db), (second, second_db)] {
        pool.close().await;
        teardown_test_db(&db).await;
    }
}

#[tokio::test]
async fn a_held_name_waits_without_a_gateway() {
    let (pool, db_name) = workspace_before_the_move(&[("phone", Some("Hall phone"), None)]).await;
    sqlx::raw_sql(MOVE_NAMES).execute(&pool).await.unwrap();
    if GATEWAY_PORT.get().is_none() {
        assert!(import_held(&pool).await.is_err());
        assert_eq!(held_names(&pool).await.unwrap().len(), 1, "nothing is lost");
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn only_a_typed_name_that_moved_reads_as_a_rename() {
    let identity = |id: &str, name: Option<&str>, label: Option<&str>| DeviceIdentity {
        id: id.into(),
        name: name.map(str::to_string),
        label: label.map(str::to_string),
    };
    let earlier = DeviceIdentities {
        devices: vec![
            identity("a", Some("A"), None),
            identity("b", None, None),
            identity("gone", Some("G"), None),
            identity("relabelled", None, None),
        ],
    };
    let now = DeviceIdentities {
        devices: vec![
            identity("a", Some("A"), Some("Safari")),
            identity("b", Some("B"), None),
            identity("new", Some("N"), None),
            identity("paired", None, Some("Chrome")),
            identity("relabelled", None, Some("Firefox")),
        ],
    };
    let renamed: Vec<String> = now
        .renamed_since(&earlier)
        .into_iter()
        .map(|d| format!("{}={}", d.id, d.name.unwrap_or_default()))
        .collect();
    assert_eq!(renamed, vec!["b=B", "gone=", "new=N"]);
    assert_eq!(
        now.identified_ids(),
        vec!["a", "b", "new", "paired", "relabelled"]
    );
}
