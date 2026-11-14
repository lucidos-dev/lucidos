//! The boot step that gives every user-entered secret one home, the
//! `credentials` table.
//!
//! Two older places held such secrets: `email_accounts.password`, a copy of
//! the `email_password` credential, and the values of `mcp_servers.env`. A
//! migration sees only their ciphertext (ADR 0509), so this step runs in Rust
//! once the secrets key is installed. It runs before the EventBus exists,
//! like a migration, so it announces nothing. Every part is idempotent: a
//! second boot finds nothing left to move.

use sqlx::PgPool;

use super::boot::column_exists;
use super::{sealed_key_id, EmailPassword, SecretColumn as _};
use crate::core::{AuthType, CredentialStore, McpServerStore};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Move both sets of values into `credentials`, then drop the mailbox
/// password column.
pub async fn move_at_boot(pool: &PgPool) -> Result<(), BoxError> {
    let mailboxes = move_mailbox_passwords(pool).await?;
    if mailboxes > 0 {
        log!("[SecretStore] Moved {mailboxes} mailbox password(s) into credentials");
    }
    let mcp = McpServerStore::move_env_into_credentials(pool).await?;
    if mcp > 0 {
        log!("[SecretStore] Moved {mcp} MCP server env value(s) into credentials");
    }
    Ok(())
}

/// Give every mailbox password with no `email_password` credential one, then
/// drop `email_accounts.password`. Returns how many it moved.
///
/// The credential takes the account's name. When another credential already
/// holds that name, it takes `email:<account>`, which
/// `CredentialStore::get_email_password` also resolves. A password that fits
/// neither name keeps the column, and the next boot tries again.
async fn move_mailbox_passwords(pool: &PgPool) -> Result<usize, BoxError> {
    if !column_exists(pool, &EmailPassword::SPEC).await? {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let rows: Vec<(String, String, String)> =
        sqlx::query_as("SELECT name, smtp_host, password FROM email_accounts WHERE password <> ''")
            .fetch_all(&mut *tx)
            .await?;
    let (mut moved, mut stranded) = (0, Vec::new());
    for (account, smtp_host, stored) in rows {
        let held: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM credentials WHERE auth_type = 'email_password' \
             AND (service_name = $1 OR service_name = 'email:' || $1))",
        )
        .bind(&account)
        .fetch_one(&mut *tx)
        .await?;
        if held {
            continue;
        }
        let password = match sealed_key_id(&stored) {
            Some(_) => super::open(&EmailPassword::SPEC, &stored)?,
            None => stored,
        };
        let scope = [format!("smtp://{smtp_host}")];
        let mut landed = false;
        for name in [account.clone(), format!("email:{account}")] {
            landed = CredentialStore::insert_moved_secret(
                &mut tx,
                &name,
                AuthType::EmailPassword,
                &scope,
                &password,
            )
            .await?;
            if landed {
                break;
            }
        }
        if landed {
            moved += 1;
        } else {
            stranded.push(account);
        }
    }
    if stranded.is_empty() {
        sqlx::query("ALTER TABLE email_accounts DROP COLUMN password")
            .execute(&mut *tx)
            .await?;
    } else {
        log!(
            "[SecretStore] Kept email_accounts.password: other credentials hold the names {} \
             and email:<name>. Rename one, and the next start moves the password.",
            stranded.join(", ")
        );
    }
    tx.commit().await?;
    Ok(moved)
}

#[cfg(test)]
#[path = "one_home_tests.rs"]
mod tests;
