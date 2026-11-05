use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use std::collections::HashMap;

use crate::core::secret_store::{self, McpServerEnv, SecretColumn as _};
use crate::core::{AuthType, CredentialStore};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, McpEnvValue>,
    pub auto_approve: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// The tool manifest as last observed, verbatim. Only a running server
    /// advertises tools, so this is what answers "what does this cost" for one
    /// that is switched off.
    pub tools: Vec<crate::mcp::types::McpTool>,
    /// When [`Self::tools`] was last read off a live server. `None` means never
    /// observed, which is NOT the same as observed-and-empty: one is unknown
    /// and the other is genuinely free.
    pub tools_observed_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Tools the user switched off, by WIRE name. Filtered out of the
    /// definitions every request carries.
    pub disabled_tools: Vec<String>,
}

/// One entry of an MCP server's environment: a value, or the name of a
/// credential the server gets the value of at launch. On the wire a value is
/// a string and a credential is `{"credential": "<name>"}`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpEnvValue {
    Plain(String),
    Credential { credential: String },
}

// Manual `Debug`, so a plain value, often an API key, never reaches a log.
impl std::fmt::Debug for McpEnvValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Plain(_) => f.write_str("Plain(<redacted>)"),
            Self::Credential { credential } => write!(f, "Credential({credential})"),
        }
    }
}

/// The columns every read selects, in the order [`row_to_server`] unpacks them.
/// A NULL `env_credentials` is a row whose values boot has not moved yet: it
/// points at nothing so far.
const SERVER_COLUMNS: &str = "id, name, command, args, env, \
                              COALESCE(env_credentials, '{}'::jsonb), auto_approve, \
                              created_at, tools, tools_observed_at, disabled_tools";

/// Raw DB row from mcp_servers, in [`SERVER_COLUMNS`] order.
type McpServerRow = (
    String,
    String,
    String,
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    bool,
    chrono::DateTime<chrono::Utc>,
    serde_json::Value,
    Option<chrono::DateTime<chrono::Utc>>,
    Vec<String>,
);

/// Longest server id that still leaves room for `mcp__<id>__<tool>` with one
/// character of tool name. Past it the wire name is truncated through the
/// separator, and no tool call parses back to a server.
pub(crate) const MAX_SERVER_ID_LEN: usize =
    crate::llm::validate::MAX_TOOL_NAME_LEN - "mcp__".len() - "__".len() - 1;

/// Reject a server id the tool-name layer could not carry.
///
/// The id is half of every wire tool name (`mcp__<id>__<tool>`), so it has to
/// survive that round trip untouched. `__` is the separator itself. Any other
/// character outside the Messages API alphabet would be rewritten, leaving a
/// server whose tools are visible but cannot be dispatched.
pub(crate) fn validate_server_id(id: &str) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let shape = "use letters, digits, '-' and single '_' (e.g. 'backstage', 'dev-docs')";
    if !crate::llm::validate::is_wire_safe_tool_name(id) {
        return Err(format!("MCP server id '{}' is not usable: {}", id, shape).into());
    }
    // A trailing `_` joins the separator: `mcp__srv___run` parses back as
    // server `srv`, tool `_run`.
    if id.contains("__") || id.ends_with('_') {
        return Err(format!(
            "MCP server id '{}' must not contain '__' or end in '_', since '__' separates the id from the tool name: {}",
            id, shape
        )
        .into());
    }
    if id.len() > MAX_SERVER_ID_LEN {
        return Err(format!(
            "MCP server id '{}' is {} characters, over the {} that fit in a tool name: {}",
            id,
            id.len(),
            MAX_SERVER_ID_LEN,
            shape
        )
        .into());
    }
    Ok(())
}

/// The stored form of a disabled-tool selection: sorted and deduped, so the
/// column holds a set. A client resending the same names in another order is
/// then not a change, and nothing re-announces.
fn normalize_disabled_tools(names: &[String]) -> Vec<String> {
    let mut set: Vec<String> = names.to_vec();
    set.sort();
    set.dedup();
    set
}

/// The registry of external MCP servers whose tools the agent can call.
///
/// **No caller can skip the event.** [`Self::register`], [`Self::unregister`],
/// [`Self::set_auto_approve`] and [`Self::set_tool_disabled`] are the reachable
/// mutators that change what the user chose; the raw row writes are private to
/// this module. Each moves the agent's own tool surface or the permission on
/// it, so each belongs on the timeline: before this, adding an MCP server from
/// chat left no trace anywhere and nothing could react to it.
///
/// [`Self::set_tools`] is the one reachable writer that stays silent, and it is
/// registered as such in `core::announced_surfaces`. It caches what a live
/// server reported rather than recording a decision, and the only event that
/// could carry it describes an auto-approve change. `tools_observed_at` is its
/// signal instead.
///
/// Same shape as `RepositoryStore`; see `core::announced_surfaces`.
pub struct McpServerStore;

fn row_to_server(row: McpServerRow) -> Result<McpServer, Box<dyn std::error::Error + Send + Sync>> {
    let (
        id,
        name,
        command,
        args,
        env,
        env_credentials,
        auto_approve,
        created_at,
        tools,
        tools_observed_at,
        disabled_tools,
    ) = row;
    let pointers = pointers_of(env_credentials)?;
    let env = secret_store::open_json_values(&McpServerEnv::SPEC, env)?
        .into_iter()
        .map(|(var, value)| (var, McpEnvValue::Plain(value)))
        .chain(
            pointers
                .into_iter()
                .map(|(var, credential)| (var, McpEnvValue::Credential { credential })),
        )
        .collect();
    Ok(McpServer {
        id,
        name,
        command,
        args: serde_json::from_value(args)?,
        env,
        auto_approve,
        created_at,
        tools: serde_json::from_value(tools)?,
        tools_observed_at,
        disabled_tools,
    })
}

/// The credential name a server's env variable `var` moves into. Only a
/// credential with this name is the server's own, to delete with it.
fn owned_credential_name(server_id: &str, var: &str) -> String {
    format!("mcp-{server_id}-{var}")
}

/// A server's stored credential pointers, by env variable.
fn pointers_of(
    env_credentials: serde_json::Value,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
    Ok(serde_json::from_value(env_credentials)?)
}

/// Serializes every write to credential pointers with the credential cleanup
/// that reads them. Held from a register's pointer check through its cleanup.
static POINTER_WRITES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Delete each credential in `dropped` that server `server_id` owns and no
/// server's pointers still name. A credential with any other name stays.
///
/// The caller holds [`POINTER_WRITES`], so no register can add a pointer
/// between the check and the delete.
async fn delete_owned_credentials(
    pool: &PgPool,
    event_bus: &EventBus,
    server_id: &str,
    dropped: HashMap<String, String>,
    actor: Option<MessageOrigin>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for (var, name) in dropped {
        if name != owned_credential_name(server_id, &var) {
            continue;
        }
        let still_named: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM mcp_servers, \
             jsonb_each_text(env_credentials) AS pointer(var, name) WHERE pointer.name = $1)",
        )
        .bind(&name)
        .fetch_one(pool)
        .await?;
        if still_named {
            continue;
        }
        if let Some(credential) = CredentialStore::get(pool, &name).await? {
            CredentialStore::delete(pool, event_bus, credential.id, actor.clone()).await?;
        }
    }
    Ok(())
}

fn missing_credential(var: &str, name: &str) -> String {
    format!("{var} names credential '{name}', and no credential has that name")
}

/// The value credential `name` gives env variable `var`, or why it gives none.
async fn credential_value(
    pool: &PgPool,
    var: &str,
    name: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let credential = CredentialStore::get(pool, name)
        .await?
        .ok_or_else(|| missing_credential(var, name))?;
    if credential.auth_type == AuthType::Password {
        return Err(format!(
            "{var} names credential '{name}', a username and password pair. \
             Point it at a credential holding one value, such as a secret or an API key"
        )
        .into());
    }
    Ok(credential.secret(pool).await?)
}

/// The environment `server` launches with: each credential it names,
/// resolved now, so a rotated key reaches the next start.
pub async fn launch_env(
    pool: &PgPool,
    server: &McpServer,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error + Send + Sync>> {
    let mut env = HashMap::new();
    for (var, value) in &server.env {
        let value = match value {
            McpEnvValue::Plain(value) => value.clone(),
            McpEnvValue::Credential { credential } => {
                credential_value(pool, var, credential).await?
            }
        };
        env.insert(var.clone(), value);
    }
    Ok(env)
}

/// What an MCP server launch spawns: a command, its arguments and its
/// environment. A setup request and a stored row each describe one.
#[derive(Clone, Copy)]
pub struct LaunchSpec<'a> {
    pub command: &'a str,
    pub args: &'a [String],
    pub env: &'a HashMap<String, McpEnvValue>,
}

/// How a launch line shows a plain env value. A credential pointer always
/// shows as `<credential NAME>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchLineMode {
    /// The value verbatim. Only the fingerprint reads a stored row this way,
    /// since a stored plain value is sealed at rest.
    Real,
    /// Each plain value as `<stored value>`, safe for a card, an event or
    /// the model.
    Masked,
}

impl LaunchSpec<'_> {
    /// The shell line this launch amounts to, for the command guard to read
    /// as a `run_bash` command.
    ///
    /// The spawn runs the command with its arguments and environment, through
    /// no shell, in `spawn_cwd`. The line states each part: a `VAR=` preamble
    /// for the environment, and a `cd` when the spawn does not run in `root`,
    /// where a bash command runs.
    pub fn command_line(
        &self,
        spawn_cwd: &std::path::Path,
        root: &std::path::Path,
        mode: LaunchLineMode,
    ) -> String {
        let mut words = Vec::new();
        if !same_directory(spawn_cwd, root) {
            words.extend(["cd".to_string(), shell_word(&spawn_cwd.to_string_lossy())]);
            words.push("&&".to_string());
        }
        let mut env: Vec<_> = self.env.iter().collect();
        env.sort_by_key(|(name, _)| name.as_str());
        for (name, value) in env {
            let value = match (value, mode) {
                (McpEnvValue::Plain(v), LaunchLineMode::Real) => shell_word(v),
                (McpEnvValue::Plain(_), LaunchLineMode::Masked) => shell_word("<stored value>"),
                (McpEnvValue::Credential { credential }, _) => {
                    shell_word(&format!("<credential {credential}>"))
                }
            };
            words.push(format!("{name}={value}"));
        }
        words.push(shell_word(self.command));
        words.extend(self.args.iter().map(|a| shell_word(a)));
        words.join(" ")
    }

    /// The *launch fingerprint*: HMAC-SHA256 of the real line under the
    /// secret store key, in hex. A changed command, argument, env value or
    /// spawn directory changes it.
    ///
    /// The key is what keeps the sealed values it covers sealed. Every other
    /// part of the line is readable, so an unkeyed hash of a short value
    /// could be guessed offline from the stored row.
    pub fn fingerprint(
        &self,
        spawn_cwd: &std::path::Path,
        root: &std::path::Path,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use hmac::Mac;
        let line = self.command_line(spawn_cwd, root, LaunchLineMode::Real);
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret_store::key()?.as_bytes())?;
        mac.update(b"mcp-launch-fingerprint\0");
        mac.update(line.as_bytes());
        Ok(crate::api::hex::hex_lower(&mac.finalize().into_bytes()))
    }
}

impl McpServer {
    pub fn launch_spec(&self) -> LaunchSpec<'_> {
        LaunchSpec {
            command: &self.command,
            args: &self.args,
            env: &self.env,
        }
    }
}

/// Whether two paths name one directory, symlinks resolved where they can be.
fn same_directory(a: &std::path::Path, b: &std::path::Path) -> bool {
    let resolve =
        |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    resolve(a) == resolve(b)
}

/// `s` as one shell word: bare when every character is inert, otherwise
/// single-quoted, with each `'` written as `'\''`.
pub(crate) fn shell_word(s: &str) -> String {
    let inert = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if !s.is_empty() && s.chars().all(inert) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

impl McpServerStore {
    /// Move the env values of every server written before credential
    /// pointers existed into `secret` credentials named `mcp-<id>-<VAR>`,
    /// leaving a pointer to each. Returns how many values moved.
    ///
    /// A boot step (`secret_store::one_home`): it runs before the EventBus
    /// exists, so it announces nothing. One transaction per server. A NULL
    /// `env_credentials` marks a server not moved yet, and the move sets it,
    /// so a value added after the move stays in `env`. A name another
    /// credential already holds keeps its value in `env`.
    pub(crate) async fn move_env_into_credentials(
        pool: &PgPool,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let unmoved: Vec<String> =
            sqlx::query_scalar("SELECT id FROM mcp_servers WHERE env_credentials IS NULL")
                .fetch_all(pool)
                .await?;
        let mut moved = 0;
        for id in unmoved {
            let mut tx = pool.begin().await?;
            let env: serde_json::Value = sqlx::query_scalar(
                "SELECT env FROM mcp_servers WHERE id = $1 AND env_credentials IS NULL FOR UPDATE",
            )
            .bind(&id)
            .fetch_one(&mut *tx)
            .await?;
            let mut kept = HashMap::new();
            let mut pointers = HashMap::new();
            for (var, value) in secret_store::open_json_values(&McpServerEnv::SPEC, env)? {
                let name = owned_credential_name(&id, &var);
                if CredentialStore::insert_moved_secret(
                    &mut tx,
                    &name,
                    AuthType::Secret,
                    &[],
                    &value,
                )
                .await?
                {
                    pointers.insert(var, name);
                    moved += 1;
                } else {
                    log!("[MCP] Kept {var} of server '{id}' in place: a credential is already named {name}");
                    kept.insert(var, value);
                }
            }
            sqlx::query("UPDATE mcp_servers SET env = $2, env_credentials = $3 WHERE id = $1")
                .bind(&id)
                .bind(secret_store::seal_json_values(&McpServerEnv::SPEC, &kept)?)
                .bind(serde_json::to_value(&pointers)?)
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
        }
        Ok(moved)
    }

    pub async fn list(
        pool: &PgPool,
    ) -> Result<Vec<McpServer>, Box<dyn std::error::Error + Send + Sync>> {
        let rows: Vec<McpServerRow> = sqlx::query_as(&format!(
            "SELECT {SERVER_COLUMNS} FROM mcp_servers ORDER BY created_at"
        ))
        .fetch_all(pool)
        .await?;

        rows.into_iter().map(row_to_server).collect()
    }

    pub async fn get(
        pool: &PgPool,
        id: &str,
    ) -> Result<Option<McpServer>, Box<dyn std::error::Error + Send + Sync>> {
        let row: Option<McpServerRow> = sqlx::query_as(&format!(
            "SELECT {SERVER_COLUMNS} FROM mcp_servers WHERE id = $1"
        ))
        .bind(id)
        .fetch_optional(pool)
        .await?;

        row.map(row_to_server).transpose()
    }

    /// Write (or rewrite) a server row. **Private on purpose**:
    /// [`Self::register`] is the reachable mutator, and it emits. See the
    /// type-level doc.
    async fn upsert_row(
        pool: &PgPool,
        id: &str,
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, McpEnvValue>,
    ) -> Result<McpServer, Box<dyn std::error::Error + Send + Sync>> {
        let args_json = serde_json::to_value(args)?;
        let mut plain = HashMap::new();
        let mut pointers = HashMap::new();
        for (var, value) in env {
            match value {
                McpEnvValue::Plain(value) => plain.insert(var.clone(), value.clone()),
                McpEnvValue::Credential { credential } => {
                    pointers.insert(var.clone(), credential.clone())
                }
            };
        }
        let env_json = secret_store::seal_json_values(&McpServerEnv::SPEC, &plain)?;

        // RETURNING the whole row, not just what was written: the ON CONFLICT
        // arm leaves auto_approve, the cached manifest and the disabled set
        // alone. A previously auto-approved server must still report `true`, or
        // the caller connects with a flag that disagrees with the DB row.
        let row: McpServerRow = sqlx::query_as(&format!(
            "INSERT INTO mcp_servers (id, name, command, args, env, env_credentials) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, command = EXCLUDED.command, \
             args = EXCLUDED.args, env = EXCLUDED.env, env_credentials = EXCLUDED.env_credentials \
             RETURNING {SERVER_COLUMNS}"
        ))
        .bind(id)
        .bind(name)
        .bind(command)
        .bind(&args_json)
        .bind(&env_json)
        .bind(serde_json::to_value(&pointers)?)
        .fetch_one(pool)
        .await?;

        row_to_server(row)
    }

    /// Delete a server row, returning the `env_credentials` it held, or
    /// `None` when no such server exists. **Private on purpose**:
    /// [`Self::unregister`] emits.
    async fn delete_row(
        pool: &PgPool,
        id: &str,
    ) -> Result<Option<serde_json::Value>, Box<dyn std::error::Error + Send + Sync>> {
        let env_credentials: Option<serde_json::Value> = sqlx::query_scalar(
            "WITH approvals AS (DELETE FROM mcp_launch_approvals WHERE server_id = $1) \
             DELETE FROM mcp_servers WHERE id = $1 \
             RETURNING COALESCE(env_credentials, '{}'::jsonb)",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(env_credentials)
    }

    /// Flip auto-approve on a server row. **Private on purpose**:
    /// [`Self::set_auto_approve`] emits.
    ///
    /// Returns `None` when no such server exists, `Some(changed)` otherwise.
    /// Both bits are needed and `rows_affected` gives neither cleanly: Postgres
    /// writes a new tuple version even when the value is identical, so an
    /// affected row means "it exists", never "it changed". The self-join reads
    /// the pre-update value in the same statement (the subselect sees the
    /// snapshot from before the write), so one round trip answers both.
    async fn set_auto_approve_row(
        pool: &PgPool,
        id: &str,
        auto_approve: bool,
    ) -> Result<Option<bool>, Box<dyn std::error::Error + Send + Sync>> {
        let changed: Option<bool> = sqlx::query_scalar(
            "UPDATE mcp_servers AS m SET auto_approve = $2 \
             FROM (SELECT id, auto_approve FROM mcp_servers WHERE id = $1) AS prior \
             WHERE m.id = prior.id \
             RETURNING (prior.auto_approve IS DISTINCT FROM $2)",
        )
        .bind(id)
        .bind(auto_approve)
        .fetch_optional(pool)
        .await?;
        Ok(changed)
    }

    /// Cache the tool manifest a live server just advertised, stamping
    /// `tools_observed_at`. Deliberately silent, see the type-level doc.
    ///
    /// Returns whether the row existed. A server removed while its process was
    /// connecting writes nothing rather than resurrecting the row, which an
    /// UPSERT here would do.
    pub async fn set_tools(
        pool: &PgPool,
        id: &str,
        tools: &[crate::mcp::types::McpTool],
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let tools_json = serde_json::to_value(tools)?;
        let result = sqlx::query(
            "UPDATE mcp_servers SET tools = $2, tools_observed_at = NOW() WHERE id = $1",
        )
        .bind(id)
        .bind(&tools_json)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Switch one of a server's tools off or back on, by wire name, and
    /// announce it. Disabling a tool takes it out of every request, so it
    /// changes the agent's own tool surface exactly as a registration does.
    ///
    /// The read and the write share one row lock, so two switches on the same
    /// server, from two clients or two tabs, both land. Each call names only
    /// its own tool, so writes commute (ADR 0437).
    ///
    /// Announces only when the set actually MOVED, the same rule
    /// [`Self::set_auto_approve`] follows.
    ///
    /// Returns the stored set, or `None` when no such server exists.
    pub async fn set_tool_disabled(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        wire_name: &str,
        disabled: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<Vec<String>>, Box<dyn std::error::Error + Send + Sync>> {
        let mut tx = pool.begin().await?;
        let current: Option<Vec<String>> =
            sqlx::query_scalar("SELECT disabled_tools FROM mcp_servers WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(current) = current else {
            return Ok(None);
        };
        let current = normalize_disabled_tools(&current);
        let mut wanted: Vec<String> = current
            .iter()
            .filter(|name| name.as_str() != wire_name)
            .cloned()
            .collect();
        if disabled {
            wanted.push(wire_name.to_string());
        }
        let wanted = normalize_disabled_tools(&wanted);
        let changed = wanted != current;
        if changed {
            sqlx::query("UPDATE mcp_servers SET disabled_tools = $2 WHERE id = $1")
                .bind(id)
                .bind(&wanted)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        if changed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::McpServerDisabledToolsChanged {
                        server_id: id.to_string(),
                        disabled_tools: wanted.clone(),
                        actor,
                    }),
                    "[MCP] McpServerDisabledToolsChanged",
                )
                .await;
        }
        Ok(Some(wanted))
    }

    /// Register (or re-register) a server and announce it. The only way to add
    /// one.
    ///
    /// Announces unconditionally, including on a re-registration: the row is
    /// rewritten with a new command / args / env, which changes what the tools
    /// actually do even when the id is unchanged.
    ///
    /// A pointer the new env drops takes its credential with it, when the
    /// server owns it and no server still names it.
    #[allow(clippy::too_many_arguments)] // one arg per server column, plus the bus and actor
    pub async fn register(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, McpEnvValue>,
        actor: Option<MessageOrigin>,
    ) -> Result<McpServer, Box<dyn std::error::Error + Send + Sync>> {
        validate_server_id(id)?;
        for (var, value) in env {
            if let McpEnvValue::Credential { credential } = value {
                credential_value(pool, var, credential).await?;
            }
        }
        // Resolving a value can wait on 1Password, so the lock comes after it.
        // A cleanup may have deleted a credential since: check names again.
        let _pointers = POINTER_WRITES.lock().await;
        for (var, value) in env {
            if let McpEnvValue::Credential { credential } = value {
                if CredentialStore::get(pool, credential).await?.is_none() {
                    return Err(missing_credential(var, credential).into());
                }
            }
        }
        let prior: Option<serde_json::Value> = sqlx::query_scalar(
            "SELECT COALESCE(env_credentials, '{}'::jsonb) FROM mcp_servers WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        let mut dropped = prior.map(pointers_of).transpose()?.unwrap_or_default();
        let server = Self::upsert_row(pool, id, name, command, args, env).await?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::McpServerRegistered {
                    server_id: server.id.clone(),
                    name: server.name.clone(),
                    actor: actor.clone(),
                }),
                "[MCP] McpServerRegistered",
            )
            .await;
        dropped.retain(|var, credential| {
            server.env.get(var)
                != Some(&McpEnvValue::Credential {
                    credential: credential.clone(),
                })
        });
        delete_owned_credentials(pool, event_bus, id, dropped, actor)
            .await
            .map_err(|e| {
                format!(
                    "Saved MCP server '{id}', but could not delete a credential it dropped: {e}"
                )
            })?;
        Ok(server)
    }

    /// Unregister a server and announce it. Announces only when a row was
    /// actually removed.
    ///
    /// Deletes each credential the server owned (`mcp-<id>-<VAR>`) that no
    /// other server names, so its secret stops being injectable. The removal
    /// is announced first: an error here means the server is gone.
    pub async fn unregister(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let _pointers = POINTER_WRITES.lock().await;
        let Some(env_credentials) = Self::delete_row(pool, id).await? else {
            return Ok(false);
        };
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::McpServerRemoved {
                    server_id: id.to_string(),
                    actor: actor.clone(),
                }),
                "[MCP] McpServerRemoved",
            )
            .await;
        let pointers = pointers_of(env_credentials);
        async { delete_owned_credentials(pool, event_bus, id, pointers?, actor).await }
            .await
            .map_err(|e| {
                format!(
                    "Removed MCP server '{id}', but could not delete the credentials it owned: {e}"
                )
            })?;
        Ok(true)
    }

    /// Set a server's auto-approve flag and announce it. This decides whether
    /// its tool calls prompt the user, so it is a permission change, not a
    /// preference.
    ///
    /// Returns whether the server exists (what callers report to the user), but
    /// announces only when the flag actually MOVED. Re-asserting the current
    /// value would otherwise put a permission-change entry on the timeline for
    /// a permission that did not change, and re-fire every
    /// `on_event: McpServerUpdated` trigger.
    pub async fn set_auto_approve(
        pool: &PgPool,
        event_bus: &EventBus,
        id: &str,
        auto_approve: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let outcome = Self::set_auto_approve_row(pool, id, auto_approve).await?;
        if outcome == Some(true) {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::McpServerUpdated {
                        server_id: id.to_string(),
                        auto_approve,
                        actor,
                    }),
                    "[MCP] McpServerUpdated",
                )
                .await;
        }
        Ok(outcome.is_some())
    }

    /// Record that `fingerprint` (see [`LaunchSpec::fingerprint`]) is an
    /// approved launch line for `server_id`, and announce it. Announces only a
    /// new approval: re-approving an approved line changes no permission.
    pub async fn approve_launch(
        pool: &PgPool,
        event_bus: &EventBus,
        server_id: &str,
        fingerprint: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let inserted = sqlx::query(
            "INSERT INTO mcp_launch_approvals (server_id, fingerprint) VALUES ($1, $2) \
             ON CONFLICT DO NOTHING",
        )
        .bind(server_id)
        .bind(fingerprint)
        .execute(pool)
        .await?
        .rows_affected()
            > 0;
        if inserted {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::McpLaunchApproved {
                        server_id: server_id.to_string(),
                        actor,
                    }),
                    "[MCP] McpLaunchApproved",
                )
                .await;
        }
        Ok(())
    }

    /// Whether someone approved `fingerprint` as `server_id`'s launch line.
    pub async fn is_launch_approved(
        pool: &PgPool,
        server_id: &str,
        fingerprint: &str,
    ) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM mcp_launch_approvals \
             WHERE server_id = $1 AND fingerprint = $2)",
        )
        .bind(server_id)
        .bind(fingerprint)
        .fetch_one(pool)
        .await
    }

    /// Every approved launch line, as `(server_id, fingerprint)` pairs.
    pub async fn launch_approvals(
        pool: &PgPool,
    ) -> Result<std::collections::HashSet<(String, String)>, sqlx::Error> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT server_id, fingerprint FROM mcp_launch_approvals")
                .fetch_all(pool)
                .await?;
        Ok(rows.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An id that cannot round-trip through `mcp__<id>__<tool>` is refused
    /// where the cause is obvious. The alternative is a server whose tools the
    /// agent can see but never call.
    #[test]
    fn server_id_must_survive_the_wire_tool_name() {
        for ok in ["backstage", "dev-docs", "roblox_studio", "bq2"] {
            assert!(validate_server_id(ok).is_ok(), "{ok} should be accepted");
        }
        for bad in ["", "back.stage", "my server", "a__b", "srv_", "café"] {
            assert!(
                validate_server_id(bad).is_err(),
                "{bad:?} should be refused"
            );
        }
    }

    /// An id can be wire-safe on its own and still leave no room for the
    /// separator plus a tool name. The composed name is then truncated through
    /// the `__`, and every tool the server advertises fails to parse back.
    #[test]
    fn the_longest_accepted_server_id_still_leaves_room_for_a_tool() {
        let longest = "a".repeat(MAX_SERVER_ID_LEN);
        assert!(validate_server_id(&longest).is_ok());
        assert!(validate_server_id(&"a".repeat(MAX_SERVER_ID_LEN + 1)).is_err());

        // The tightest wire name that id can produce still round-trips.
        let wire = crate::llm::validate::wire_safe_tool_name(&format!("mcp__{longest}__x"));
        assert!(crate::llm::validate::is_wire_safe_tool_name(&wire));
        assert_eq!(
            crate::mcp::McpManager::parse_mcp_tool_name(&wire),
            Some((longest, "x".to_string()))
        );
    }

    fn spec_env() -> HashMap<String, McpEnvValue> {
        HashMap::from([
            (
                "API_KEY".to_string(),
                McpEnvValue::Plain("sk-live-1".to_string()),
            ),
            (
                "TOKEN".to_string(),
                McpEnvValue::Credential {
                    credential: "gh".to_string(),
                },
            ),
        ])
    }

    #[test]
    fn the_masked_launch_line_never_holds_a_plain_value() {
        let ws = tempfile::tempdir().unwrap();
        let args = vec!["s.js".to_string()];
        let env = spec_env();
        let spec = LaunchSpec {
            command: "node",
            args: &args,
            env: &env,
        };
        let masked = spec.command_line(ws.path(), ws.path(), LaunchLineMode::Masked);
        assert_eq!(
            masked,
            "API_KEY='<stored value>' TOKEN='<credential gh>' node s.js"
        );
        assert!(!masked.contains("sk-live-1"));
        assert_eq!(
            spec.command_line(ws.path(), ws.path(), LaunchLineMode::Real),
            "API_KEY=sk-live-1 TOKEN='<credential gh>' node s.js"
        );
    }

    /// Re-registering with any part changed needs a new approval.
    #[test]
    fn every_part_of_a_launch_moves_its_fingerprint() {
        let ws = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let root = ws.path();
        let args = vec!["s.js".to_string()];
        let other_args = vec!["t.js".to_string()];
        let env = spec_env();
        let mut other_env = spec_env();
        other_env.insert(
            "API_KEY".to_string(),
            McpEnvValue::Plain("sk-live-2".to_string()),
        );
        let base = LaunchSpec {
            command: "node",
            args: &args,
            env: &env,
        };
        let print = base.fingerprint(root, root).unwrap();
        assert_eq!(print, base.fingerprint(root, root).unwrap(), "stable");
        assert_eq!(print.len(), 64);
        let variants = [
            LaunchSpec {
                command: "deno",
                ..base
            }
            .fingerprint(root, root),
            LaunchSpec {
                args: &other_args,
                ..base
            }
            .fingerprint(root, root),
            LaunchSpec {
                env: &other_env,
                ..base
            }
            .fingerprint(root, root),
            base.fingerprint(elsewhere.path(), root),
        ];
        for variant in variants {
            assert_ne!(variant.unwrap(), print);
        }
    }

    /// A database reader sees the row and its fingerprint. Without the key,
    /// hashing guessed lines must not reproduce the fingerprint.
    #[test]
    fn a_launch_fingerprint_is_keyed() {
        use sha2::{Digest, Sha256};
        let ws = tempfile::tempdir().unwrap();
        let args = vec!["s.js".to_string()];
        let env = spec_env();
        let spec = LaunchSpec {
            command: "node",
            args: &args,
            env: &env,
        };
        let line = spec.command_line(ws.path(), ws.path(), LaunchLineMode::Real);
        let unkeyed = crate::api::hex::hex_lower(&Sha256::digest(line.as_bytes()));
        assert_ne!(spec.fingerprint(ws.path(), ws.path()).unwrap(), unkeyed);
    }

    /// An approval is a permission decision: it lands on the timeline with who
    /// made it. Re-approving stays silent, and removing the server drops its
    /// approvals, so a later server under the same id must ask again.
    #[tokio::test]
    async fn a_launch_approval_announces_with_its_actor() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &HashMap::new(), None)
            .await
            .unwrap();
        let actor = MessageOrigin::Device {
            device_id: "dev-1".into(),
        };
        assert!(!McpServerStore::is_launch_approved(&pool, "srv", "fp")
            .await
            .unwrap());

        for _ in 0..2 {
            McpServerStore::approve_launch(&pool, &bus, "srv", "fp", Some(actor.clone()))
                .await
                .unwrap();
        }
        assert!(McpServerStore::is_launch_approved(&pool, "srv", "fp")
            .await
            .unwrap());
        assert!(!McpServerStore::is_launch_approved(&pool, "srv", "other")
            .await
            .unwrap());
        let actors: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT payload->'data'->'actor'->>'device_id' FROM events \
             WHERE event_type = 'McpLaunchApproved' AND aggregate_id = 'srv'",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(actors, vec![Some("dev-1".to_string())]);

        McpServerStore::unregister(&pool, &bus, "srv", None)
            .await
            .unwrap();
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &HashMap::new(), None)
            .await
            .unwrap();
        assert!(!McpServerStore::is_launch_approved(&pool, "srv", "fp")
            .await
            .unwrap());
        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Registering an MCP server changes the agent's own tool surface, and
    /// auto-approve decides whether those tools prompt. Both now reach the
    /// timeline; before this they were written silently, so asking the agent in
    /// chat to add a server left no trace anywhere.
    #[tokio::test]
    async fn every_registry_mutation_announces() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let env = HashMap::new();
        async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
                .bind(event_type)
                .fetch_one(pool)
                .await
                .unwrap()
        }

        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "McpServerRegistered").await, 1);

        // A re-registration rewrites command / args / env, so what the tools do
        // changed even though the id did not.
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd2", &[], &env, None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "McpServerRegistered").await, 2);

        assert!(
            McpServerStore::set_auto_approve(&pool, &bus, "srv", true, None)
                .await
                .unwrap()
        );
        assert_eq!(emitted(&pool, "McpServerUpdated").await, 1);

        // Re-asserting the value the flag already holds still reports that the
        // server exists, but must not announce: a Postgres UPDATE writes a new
        // tuple version regardless of value equality, so rows_affected would
        // say "changed" for a permission that did not change.
        assert!(
            McpServerStore::set_auto_approve(&pool, &bus, "srv", true, None)
                .await
                .unwrap(),
            "the server still exists"
        );
        assert_eq!(
            emitted(&pool, "McpServerUpdated").await,
            1,
            "a no-op toggle must not announce a permission change"
        );

        assert!(
            !McpServerStore::set_auto_approve(&pool, &bus, "missing", true, None)
                .await
                .unwrap()
        );
        assert_eq!(
            emitted(&pool, "McpServerUpdated").await,
            1,
            "a toggle that matched no row must not announce"
        );

        assert!(McpServerStore::unregister(&pool, &bus, "srv", None)
            .await
            .unwrap());
        assert_eq!(emitted(&pool, "McpServerRemoved").await, 1);
        assert!(!McpServerStore::unregister(&pool, &bus, "srv", None)
            .await
            .unwrap());
        assert_eq!(
            emitted(&pool, "McpServerRemoved").await,
            1,
            "second unregister removes nothing and therefore announces nothing"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    fn tool(name: &str) -> crate::mcp::types::McpTool {
        crate::mcp::types::McpTool {
            name: name.to_string(),
            description: Some(format!("does {name}")),
            input_schema: Some(serde_json::json!({"type": "object", "properties": {}})),
        }
    }

    /// The manifest has to survive the process that reported it. Reading cost
    /// back for a switched-off server is the whole reason the column exists.
    #[tokio::test]
    async fn the_tool_manifest_round_trips_and_never_observed_is_not_empty() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let env = HashMap::new();

        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();

        // A fresh row is "never observed", which is NOT an empty manifest: the
        // engine does not know what this server offers.
        let fresh = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
        assert!(fresh.tools.is_empty());
        assert!(
            fresh.tools_observed_at.is_none(),
            "nothing has connected yet, so there is no observation to stamp"
        );

        let observed = [tool("alpha"), tool("beta")];
        assert!(McpServerStore::set_tools(&pool, "srv", &observed)
            .await
            .unwrap());

        let cached = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
        assert_eq!(
            cached.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );
        assert_eq!(cached.tools[0].description.as_deref(), Some("does alpha"));
        assert!(
            cached.tools[0].input_schema.is_some(),
            "the schema is most of a definition's cost, so it has to survive"
        );
        assert!(cached.tools_observed_at.is_some());

        // Observed-and-empty is a third state, distinct from never-observed:
        // this server really does cost nothing.
        assert!(McpServerStore::set_tools(&pool, "srv", &[]).await.unwrap());
        let emptied = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
        assert!(emptied.tools.is_empty());
        assert!(emptied.tools_observed_at.is_some());

        // A row that is gone writes nothing rather than resurrecting itself.
        assert!(!McpServerStore::set_tools(&pool, "missing", &observed)
            .await
            .unwrap());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Re-registering a server must not drop the manifest or the disabled set:
    /// the ON CONFLICT arm leaves both alone, so the returned struct has to
    /// report them rather than defaulting.
    #[tokio::test]
    async fn reregistering_keeps_the_manifest_and_the_disabled_set() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let env = HashMap::new();

        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();
        McpServerStore::set_tools(&pool, "srv", &[tool("alpha")])
            .await
            .unwrap();
        McpServerStore::set_tool_disabled(&pool, &bus, "srv", "mcp__srv__alpha", true, None)
            .await
            .unwrap();

        let again = McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd2", &[], &env, None)
            .await
            .unwrap();
        assert_eq!(again.tools.len(), 1, "the manifest survives a re-register");
        assert!(again.tools_observed_at.is_some());
        assert_eq!(again.disabled_tools, vec!["mcp__srv__alpha".to_string()]);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Two switches on one server both land, whatever their timing. The test
    /// holds the row lock itself, starts a switch, and commits a change of its
    /// own only once that switch waits on the lock. A switch that read the set
    /// before locking would then write it back without that change.
    #[tokio::test]
    async fn two_switches_on_one_server_both_land() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &HashMap::new(), None)
            .await
            .unwrap();

        let mut holder = pool.begin().await.unwrap();
        sqlx::query("SELECT 1 FROM mcp_servers WHERE id = 'srv' FOR UPDATE")
            .execute(&mut *holder)
            .await
            .unwrap();
        let switch = tokio::spawn({
            let (pool, bus) = (pool.clone(), bus.clone());
            async move {
                McpServerStore::set_tool_disabled(&pool, &bus, "srv", "mcp__srv__beta", true, None)
                    .await
                    .unwrap()
            }
        });
        // The synchronization point: the switch is blocked on the row lock, or
        // it finished without ever taking it.
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity \
                  WHERE datname = current_database() AND wait_event_type = 'Lock'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting > 0 || switch.is_finished() {
                break;
            }
            tokio::task::yield_now().await;
        }
        sqlx::query(
            "UPDATE mcp_servers SET disabled_tools = ARRAY['mcp__srv__alpha'] WHERE id = 'srv'",
        )
        .execute(&mut *holder)
        .await
        .unwrap();
        holder.commit().await.unwrap();

        let both = vec!["mcp__srv__alpha".to_string(), "mcp__srv__beta".to_string()];
        assert_eq!(switch.await.unwrap(), Some(both.clone()));
        let stored: Vec<String> =
            sqlx::query_scalar("SELECT disabled_tools FROM mcp_servers WHERE id = 'srv'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(stored, both);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Switching a tool off narrows the agent's own tool surface, so it goes on
    /// the timeline. Caching a manifest does not: that is an observation, and
    /// the only candidate event describes an auto-approve change.
    #[tokio::test]
    async fn disabling_a_tool_announces_but_caching_a_manifest_does_not() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let env = HashMap::new();
        async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
                .bind(event_type)
                .fetch_one(pool)
                .await
                .unwrap()
        }

        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();
        McpServerStore::set_tools(&pool, "srv", &[tool("alpha")])
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "McpServerUpdated").await, 0);
        assert_eq!(emitted(&pool, "McpServerDisabledToolsChanged").await, 0);

        let switch = |wire: &'static str, disabled: bool| {
            let (pool, bus) = (pool.clone(), bus.clone());
            async move {
                McpServerStore::set_tool_disabled(&pool, &bus, "srv", wire, disabled, None)
                    .await
                    .unwrap()
            }
        };
        switch("mcp__srv__beta", true)
            .await
            .expect("the server exists");
        let stored = switch("mcp__srv__alpha", true)
            .await
            .expect("the server exists");
        assert_eq!(
            stored,
            vec!["mcp__srv__alpha".to_string(), "mcp__srv__beta".to_string()],
            "stored as a set, in one order whatever the order of the switches"
        );
        assert_eq!(emitted(&pool, "McpServerDisabledToolsChanged").await, 2);

        // Switching a tool off that is already off changes nothing.
        assert!(switch("mcp__srv__alpha", true).await.is_some());
        assert_eq!(
            emitted(&pool, "McpServerDisabledToolsChanged").await,
            2,
            "re-sending the current state changed nothing"
        );

        // Switching it back on IS a change: the tool comes back.
        let stored = switch("mcp__srv__alpha", false)
            .await
            .expect("the server exists");
        assert_eq!(stored, vec!["mcp__srv__beta".to_string()]);
        assert_eq!(emitted(&pool, "McpServerDisabledToolsChanged").await, 3);

        assert!(
            McpServerStore::set_tool_disabled(&pool, &bus, "missing", "mcp__x__y", true, None)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            emitted(&pool, "McpServerDisabledToolsChanged").await,
            3,
            "a write that matched no row must not announce"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A pointer must name a credential holding one value, and the launch
    /// reads that value then, so a rotated key reaches the next start.
    #[tokio::test]
    async fn a_credential_pointer_is_checked_at_registration_and_read_at_launch() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let pointer = |name: &str| {
            HashMap::from([(
                "API_KEY".to_string(),
                McpEnvValue::Credential {
                    credential: name.to_string(),
                },
            )])
        };
        let save = |name: &'static str, auth_type: AuthType, value: &'static str| {
            let (pool, bus) = (pool.clone(), bus.clone());
            async move {
                CredentialStore::upsert(&pool, &bus, name, &[], auth_type, value, None, None, None)
                    .await
                    .unwrap();
            }
        };

        let missing = McpServerStore::register(
            &pool,
            &bus,
            "srv",
            "Srv",
            "cmd",
            &[],
            &pointer("absent"),
            None,
        )
        .await
        .unwrap_err();
        assert!(
            missing.to_string().contains("no credential has that name"),
            "{missing}"
        );

        save(
            "login",
            AuthType::Password,
            r#"{"username":"u","password":"p"}"#,
        )
        .await;
        let pair = McpServerStore::register(
            &pool,
            &bus,
            "srv",
            "Srv",
            "cmd",
            &[],
            &pointer("login"),
            None,
        )
        .await
        .unwrap_err();
        assert!(pair.to_string().contains("username and password"), "{pair}");

        save("mcp-key", AuthType::Secret, "sk-first").await;
        let server = McpServerStore::register(
            &pool,
            &bus,
            "srv",
            "Srv",
            "cmd",
            &[],
            &pointer("mcp-key"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(server.env, pointer("mcp-key"));
        save("mcp-key", AuthType::Secret, "sk-rotated").await;
        let env = launch_env(&pool, &server).await.unwrap();
        assert_eq!(env.get("API_KEY").map(String::as_str), Some("sk-rotated"));

        crate::test_support::teardown_test_db(&db_name).await;
    }

    fn pointers(entries: &[(&str, &str)]) -> HashMap<String, McpEnvValue> {
        entries
            .iter()
            .map(|(var, credential)| {
                (
                    var.to_string(),
                    McpEnvValue::Credential {
                        credential: credential.to_string(),
                    },
                )
            })
            .collect()
    }

    async fn save_secret(pool: &PgPool, bus: &EventBus, name: &str) {
        CredentialStore::upsert(
            pool,
            bus,
            name,
            &[],
            AuthType::Secret,
            "sk",
            None,
            None,
            None,
        )
        .await
        .unwrap();
    }

    async fn stored(pool: &PgPool, name: &str) -> bool {
        CredentialStore::get(pool, name).await.unwrap().is_some()
    }

    /// Removing a server deletes the credential its value moved into, and
    /// announces it, so the secret is no longer injectable.
    #[tokio::test]
    async fn removing_a_server_deletes_the_credential_it_owns() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        save_secret(&pool, &bus, "mcp-srv-API_KEY").await;
        let env = pointers(&[("API_KEY", "mcp-srv-API_KEY")]);
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();

        assert!(McpServerStore::unregister(&pool, &bus, "srv", None)
            .await
            .unwrap());
        assert!(!stored(&pool, "mcp-srv-API_KEY").await);
        let deleted: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type = 'CredentialDeleted'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(deleted, 1);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A cleanup that fails after the row is gone still announces the
    /// removal, and its error says the server was removed.
    #[tokio::test]
    async fn a_failed_cleanup_still_announces_the_removal() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &HashMap::new(), None)
            .await
            .unwrap();
        sqlx::query("UPDATE mcp_servers SET env_credentials = '\"unreadable\"' WHERE id = 'srv'")
            .execute(&pool)
            .await
            .unwrap();

        let error = McpServerStore::unregister(&pool, &bus, "srv", None)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("Removed MCP server 'srv'"),
            "{error}"
        );
        assert!(McpServerStore::get(&pool, "srv").await.unwrap().is_none());
        let removed: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = 'McpServerRemoved'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(removed, 1);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A credential the user named is theirs, so removing a server that
    /// points at it leaves it stored.
    #[tokio::test]
    async fn removing_a_server_keeps_a_credential_the_user_named() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        save_secret(&pool, &bus, "github-token").await;
        let env = pointers(&[("API_KEY", "github-token")]);
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &env, None)
            .await
            .unwrap();

        McpServerStore::unregister(&pool, &bus, "srv", None)
            .await
            .unwrap();
        assert!(stored(&pool, "github-token").await);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A credential another server still points at stays, or that server
    /// fails to launch.
    #[tokio::test]
    async fn removing_a_server_keeps_a_credential_another_server_names() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        save_secret(&pool, &bus, "mcp-srv-API_KEY").await;
        let env = pointers(&[("API_KEY", "mcp-srv-API_KEY")]);
        for id in ["srv", "other"] {
            McpServerStore::register(&pool, &bus, id, id, "cmd", &[], &env, None)
                .await
                .unwrap();
        }

        McpServerStore::unregister(&pool, &bus, "srv", None)
            .await
            .unwrap();
        assert!(stored(&pool, "mcp-srv-API_KEY").await);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A re-register whose env drops a pointer deletes the credential the
    /// server owned, and keeps the ones it still names.
    #[tokio::test]
    async fn reregistering_without_a_pointer_deletes_its_credential() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        save_secret(&pool, &bus, "mcp-srv-API_KEY").await;
        save_secret(&pool, &bus, "mcp-srv-TOKEN").await;
        let both = pointers(&[("API_KEY", "mcp-srv-API_KEY"), ("TOKEN", "mcp-srv-TOKEN")]);
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &both, None)
            .await
            .unwrap();

        let one = pointers(&[("TOKEN", "mcp-srv-TOKEN")]);
        McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &[], &one, None)
            .await
            .unwrap();
        assert!(!stored(&pool, "mcp-srv-API_KEY").await);
        assert!(stored(&pool, "mcp-srv-TOKEN").await);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn reinsert_preserves_auto_approve() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;

        let args = vec!["--flag".to_string()];
        let env = HashMap::new();

        // A brand-new server defaults to auto_approve = false.
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let created = McpServerStore::register(&pool, &bus, "srv", "Srv", "cmd", &args, &env, None)
            .await
            .unwrap();
        assert!(
            !created.auto_approve,
            "new server should default to auto_approve = false"
        );

        // User opts the server into auto-approve.
        assert!(
            McpServerStore::set_auto_approve(&pool, &bus, "srv", true, None)
                .await
                .unwrap()
        );

        // Re-registering the same id hits ON CONFLICT DO UPDATE, which does NOT
        // touch auto_approve. The returned struct must reflect the real DB value
        // (true), not a hardcoded false — this is the regression guard for the
        // bug where a re-register silently disabled auto-approve in memory.
        let reinserted =
            McpServerStore::register(&pool, &bus, "srv", "Srv Renamed", "cmd2", &args, &env, None)
                .await
                .unwrap();
        assert!(
            reinserted.auto_approve,
            "re-registering an auto-approved server must keep auto_approve = true"
        );
        assert_eq!(
            reinserted.name, "Srv Renamed",
            "ON CONFLICT should update the mutable fields"
        );

        // The DB row agrees — no in-memory/DB divergence.
        let fetched = McpServerStore::get(&pool, "srv").await.unwrap().unwrap();
        assert!(fetched.auto_approve);

        crate::test_support::teardown_test_db(&db_name).await;
    }
}
