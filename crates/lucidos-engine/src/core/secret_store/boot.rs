//! The boot check: find this database's key, or refuse to start (ADR 0509).
//!
//! The database says which key it needs: every sealed value carries its key
//! id. So the check compares those ids against the key store and never
//! guesses. A sealed database with no matching key never boots, and no path
//! but `lucidos-engine secrets discard` drops a secret.

use sqlx::{PgPool, Row};

use super::key_store::{parked_key_name, secrets_key_name, KeyStore};
use super::{
    Discard, SecretColumnSpec, SecretKey, Shape, MOVED_SECRET_COLUMNS, SEALED_PREFIX,
    SECRET_COLUMNS,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What the boot check decided.
#[derive(Debug)]
pub enum Boot {
    /// Install this key and serve.
    Ready(SecretKey),
    /// Starting would lose secrets. The message says why and how to recover.
    Refused(String),
}

/// The engine's boot step: run [`boot`] against this engine's key store and
/// install the key. `Some` is a refusal message for the boot-failure report.
pub async fn load_workspace_key(
    pool: &PgPool,
    database_url: &str,
) -> Result<Option<String>, BoxError> {
    let database = database_name(database_url)?;
    let store = super::key_store::resolve()?;
    match boot(pool, &database, store.as_ref()).await? {
        Boot::Ready(key) => {
            super::install(key)?;
            Ok(None)
        }
        Boot::Refused(message) => Ok(Some(message)),
    }
}

/// The database a URL names: what the workspace key is filed under.
pub fn database_name(database_url: &str) -> Result<String, BoxError> {
    Ok(lucidos_pg_url::PgUrl::parse(database_url)?.database)
}

/// Check the key, mint one for a database with nothing sealed yet, then seal
/// every plain value and validate each column's `CHECK` constraint.
pub async fn boot(pool: &PgPool, database: &str, store: &dyn KeyStore) -> Result<Boot, BoxError> {
    let name = secrets_key_name(database);
    let needed = sealed_key_ids(pool).await?;
    let key = match store.get(&name)? {
        Some(key) if needed.iter().all(|id| *id == key.id()) => key,
        found => match parked_key(store, &name, &needed)? {
            Some(parked) => parked,
            None => match found {
                Some(key) => key,
                None if needed.is_empty() => mint(store, &name)?,
                None => {
                    return Ok(Boot::Refused(refusal(
                        &needed,
                        &format!("{} holds no key", store.describe(&name)),
                    )))
                }
            },
        },
    };
    let foreign: Vec<String> = needed.into_iter().filter(|id| *id != key.id()).collect();
    if !foreign.is_empty() {
        let found = format!("{} holds key {}", store.describe(&name), key.id());
        return Ok(Boot::Refused(refusal(&foreign, &found)));
    }
    let sealed = seal_plain_values(pool, &key).await?;
    if sealed > 0 {
        log!("[SecretStore] Sealed {sealed} stored secret(s) that were plain text");
    }
    validate_constraints(pool).await?;
    Ok(Boot::Ready(key))
}

/// The key this database needs, when a restore or an import parked it. Two
/// workspaces on two clusters can share a key name, so the other one's restore
/// parks this one's key. Only a database sealed with one key can name it.
fn parked_key(
    store: &dyn KeyStore,
    name: &str,
    needed: &[String],
) -> Result<Option<SecretKey>, BoxError> {
    let [id] = needed else {
        return Ok(None);
    };
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let parked = parked_key_name(name, id);
    let key = store.get(&parked)?.filter(|key| key.id() == *id);
    if key.is_some() {
        log!(
            "[SecretStore] Using the key parked in {}",
            store.describe(&parked)
        );
    }
    Ok(key)
}

/// Mint a key, and use it only once it reads back equal. A store that drops
/// writes must never leave secrets sealed with a lost key. A rival that minted
/// first wins, and this call uses its key.
fn mint(store: &dyn KeyStore, name: &str) -> Result<SecretKey, BoxError> {
    let key = SecretKey::generate();
    let created = store.put_new(name, &key)?;
    match store.get(name)? {
        Some(read_back) if !created || read_back == key => {
            if created {
                log!(
                    "[SecretStore] Created key {} in {}",
                    key.id(),
                    store.describe(name)
                );
            }
            Ok(read_back)
        }
        _ => Err(format!("{} did not keep the new key", store.describe(name)).into()),
    }
}

fn refusal(needed: &[String], found: &str) -> String {
    format!(
        "This workspace's stored secrets are sealed with key {needed}, but {found}. \
         Lucidos will not start, because it would lose every stored credential and sign-in. \
         Put the key back with `lucidos-engine secrets import-key --key-file <file>`, \
         or remove the unreadable secrets with `lucidos-engine secrets discard`. \
         Both read the workspace database from DATABASE_URL.",
        needed = needed.join(", ")
    )
}

/// Every key id a sealed value in the database names, sorted.
pub async fn sealed_key_ids(pool: &PgPool) -> Result<Vec<String>, BoxError> {
    let mut ids = std::collections::BTreeSet::new();
    for column in present_secret_columns(pool).await? {
        let query = format!(
            "SELECT DISTINCT split_part(v, ':', 2) FROM {values} WHERE v LIKE '{SEALED_PREFIX}%'",
            values = values_of(&column)
        );
        ids.extend(
            sqlx::query_scalar::<_, String>(&query)
                .fetch_all(pool)
                .await?,
        );
    }
    Ok(ids.into_iter().collect())
}

/// Every secret column this database holds: [`SECRET_COLUMNS`], plus each
/// [`MOVED_SECRET_COLUMNS`] entry that boot has not moved and dropped yet.
async fn present_secret_columns(pool: &PgPool) -> Result<Vec<SecretColumnSpec>, BoxError> {
    let mut columns = SECRET_COLUMNS.to_vec();
    for column in MOVED_SECRET_COLUMNS {
        if column_exists(pool, column).await? {
            columns.push(*column);
        }
    }
    Ok(columns)
}

/// Whether `column` is still in the schema.
pub(crate) async fn column_exists(
    pool: &PgPool,
    column: &SecretColumnSpec,
) -> Result<bool, BoxError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM information_schema.columns \
         WHERE table_schema = 'public' AND table_name = $1 AND column_name = $2)",
    )
    .bind(column.table)
    .bind(column.column)
    .fetch_one(pool)
    .await?)
}

/// A `FROM` clause yielding one `v` per stored value of `column`.
fn values_of(column: &SecretColumnSpec) -> String {
    let (table, col) = (column.table, column.column);
    match column.shape {
        Shape::Text => format!("(SELECT {col} AS v FROM {table}) s"),
        Shape::JsonValues => format!("{table}, jsonb_each_text({table}.{col}) AS e(k, v)"),
    }
}

/// Seal every plain value in place, in one transaction. Returns how many.
///
/// Each row takes one `UPDATE` setting all its secret columns. Postgres checks
/// every `CHECK` on the new row, so sealing one column while a sibling stays
/// plain is refused.
pub(crate) async fn seal_plain_values(pool: &PgPool, key: &SecretKey) -> Result<usize, BoxError> {
    let mut tx = pool.begin().await?;
    let mut sealed = 0;
    for table in secret_tables() {
        let columns: Vec<&SecretColumnSpec> =
            SECRET_COLUMNS.iter().filter(|c| c.table == table).collect();
        let selected = joined(&columns, ", ", |_, c| format!("{}::text", c.column));
        let plain = joined(&columns, " OR ", |_, c| holds_plain(c));
        let rows = sqlx::query(&format!(
            "SELECT ctid::text, {selected} FROM {table} WHERE {plain} FOR UPDATE"
        ))
        .fetch_all(&mut *tx)
        .await?;
        let assignments = joined(&columns, ", ", |i, c| {
            let cast = match c.shape {
                Shape::Text => "",
                Shape::JsonValues => "::jsonb",
            };
            format!("{} = ${}{cast}", c.column, i + 1)
        });
        let update = format!(
            "UPDATE {table} SET {assignments} WHERE ctid = ${}::tid",
            columns.len() + 1
        );
        for row in rows {
            let mut query = sqlx::query(&update);
            for (i, column) in columns.iter().enumerate() {
                let stored: Option<String> = row.try_get(i + 1)?;
                let value = match stored {
                    Some(stored) => {
                        let (value, count) = seal_stored(key, column, stored)?;
                        sealed += count;
                        Some(value)
                    }
                    None => None,
                };
                query = query.bind(value);
            }
            query
                .bind(row.try_get::<String, _>(0)?)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;
    Ok(sealed)
}

/// Every table holding a secret column, once each, in [`SECRET_COLUMNS`] order.
fn secret_tables() -> Vec<&'static str> {
    let mut tables = Vec::new();
    for column in SECRET_COLUMNS {
        if !tables.contains(&column.table) {
            tables.push(column.table);
        }
    }
    tables
}

fn joined(
    columns: &[&SecretColumnSpec],
    separator: &str,
    part: impl Fn(usize, &SecretColumnSpec) -> String,
) -> String {
    columns
        .iter()
        .enumerate()
        .map(|(i, column)| part(i, column))
        .collect::<Vec<_>>()
        .join(separator)
}

/// A SQL condition, true when `column` holds a value still to seal.
fn holds_plain(column: &SecretColumnSpec) -> String {
    let col = column.column;
    match column.shape {
        Shape::Text => {
            // `''` is plain too where the column cannot read it as "none".
            let empty_is_plain = if column.empty_means_none {
                "FALSE"
            } else {
                "TRUE"
            };
            format!("({col} NOT LIKE '{SEALED_PREFIX}%' AND ({col} <> '' OR {empty_is_plain}))")
        }
        Shape::JsonValues => format!(
            "EXISTS (SELECT 1 FROM jsonb_each({col}) AS e(k, v) \
             WHERE jsonb_typeof(v) <> 'string' OR v #>> '{{}}' NOT LIKE '{SEALED_PREFIX}%')"
        ),
    }
}

/// `stored` with every plain value in it sealed, and how many that was.
fn seal_stored(
    key: &SecretKey,
    column: &SecretColumnSpec,
    stored: String,
) -> Result<(String, usize), BoxError> {
    match column.shape {
        Shape::Text if stored.starts_with(SEALED_PREFIX) => Ok((stored, 0)),
        Shape::Text if stored.is_empty() && column.empty_means_none => Ok((stored, 0)),
        Shape::Text => Ok((key.seal(column, &stored)?, 1)),
        Shape::JsonValues => {
            let serde_json::Value::Object(entries) = serde_json::from_str(&stored)? else {
                return Err(format!("{} holds a non-object", column.name()).into());
            };
            let mut count = 0;
            let mut resealed = serde_json::Map::new();
            for (name, entry) in entries {
                let text = match entry {
                    serde_json::Value::String(text) => text,
                    other => other.to_string(),
                };
                let text = if text.starts_with(SEALED_PREFIX) {
                    text
                } else {
                    count += 1;
                    key.seal(column, &text)?
                };
                resealed.insert(name, text.into());
            }
            Ok((serde_json::Value::Object(resealed).to_string(), count))
        }
    }
}

/// Validate each column's `CHECK`. The migration added it `NOT VALID`, so it
/// bound new writes before the old rows were sealed.
async fn validate_constraints(pool: &PgPool) -> Result<(), BoxError> {
    for column in SECRET_COLUMNS {
        sqlx::query(&format!(
            "ALTER TABLE {} VALIDATE CONSTRAINT {}",
            column.table,
            column.constraint()
        ))
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Install `key` for `database`, but only when every sealed value in it names
/// that key, so the import never strands another one.
pub async fn import_key(
    pool: &PgPool,
    database: &str,
    store: &dyn KeyStore,
    key: &SecretKey,
) -> Result<(), BoxError> {
    let foreign: Vec<String> = sealed_key_ids(pool)
        .await?
        .into_iter()
        .filter(|id| *id != key.id())
        .collect();
    if !foreign.is_empty() {
        return Err(format!(
            "key {} does not open this database: its secrets are sealed with key {}",
            key.id(),
            foreign.join(", ")
        )
        .into());
    }
    let name = secrets_key_name(database);
    if let Some(parked) = super::key_store::replace_keeping(store, &name, key)? {
        log!(
            "[SecretStore] Kept the key {} replaced as {}",
            name,
            store.describe(&parked)
        );
    }
    Ok(())
}

/// Remove every secret the workspace key cannot open, minting a key when
/// there is none. Returns one line per column it changed.
pub async fn discard(
    pool: &PgPool,
    database: &str,
    store: &dyn KeyStore,
) -> Result<Vec<String>, BoxError> {
    let name = secrets_key_name(database);
    let key = match store.get(&name)? {
        Some(key) => key,
        None => mint(store, &name)?,
    };
    let columns = present_secret_columns(pool).await?;
    let mut tx = pool.begin().await?;
    let mut report = Vec::new();
    for column in &columns {
        let (table, col) = (column.table, column.column);
        let foreign =
            |v: &str| format!("{v} LIKE '{SEALED_PREFIX}%' AND split_part({v}, ':', 2) <> $1");
        let statement = match column.discard {
            Discard::DeleteRow => format!("DELETE FROM {table} WHERE {}", foreign(col)),
            Discard::SetEmpty => {
                format!("UPDATE {table} SET {col} = '' WHERE {}", foreign(col))
            }
            Discard::DropEntry => format!(
                "UPDATE {table} SET {col} = ( \
                   SELECT coalesce(jsonb_object_agg(k, v), '{{}}'::jsonb) \
                   FROM jsonb_each({col}) AS e(k, v) WHERE NOT ({entry})) \
                 WHERE EXISTS (SELECT 1 FROM jsonb_each({col}) AS e(k, v) WHERE {entry})",
                entry = foreign("(v #>> '{}')")
            ),
        };
        let changed = sqlx::query(&statement)
            .bind(key.id())
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if changed > 0 {
            report.push(format!(
                "{}: {changed} row(s), {:?}",
                column.name(),
                column.discard
            ));
        }
    }
    tx.commit().await?;
    Ok(report)
}

#[cfg(test)]
#[path = "boot_tests.rs"]
mod tests;
