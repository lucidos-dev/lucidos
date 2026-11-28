//! INVARIANT: *Standing denies*: the command rules the owner sets with Always deny.
//!
//! A standing deny is a line of the [`GrantFile::AgentDeniedCommands`] lane:
//! the line an Always allow would store, held as a workspace `permissions`
//! row. It matches a command by name, so it is a command rule and protects no
//! file.
//!
//! Callers must read denies before any allow and before the judge, and after
//! the floor. A deny never weakens the floor or a private path.

use uuid::Uuid;

use super::tool_grants::{self, AnsweredBy, ChangedBy, StandingChange};
use crate::core::grants::{GrantFile, GrantSource};
use crate::engine::command_guard;
use crate::engine::event_bus::EventBus;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const ANY_SHELL: &str = "Bash";
const ANY_PYTHON: &str = "Python";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    Shell,
    Python,
}

fn denied_head(line: &str) -> Option<&str> {
    line.strip_prefix("Bash(")?.strip_suffix(":*)")
}

/// INVARIANT: The deny among `denies` that covers `command`, if any.
///
/// A shell command is denied when ANY segment runs a denied program. An allow
/// needs every segment covered, so the two differ on purpose: one denied
/// program refuses the whole line.
pub fn matching<'d>(denies: &'d [String], kind: CommandKind, command: &str) -> Option<&'d str> {
    let holds = |line: &str| denies.iter().find(|d| *d == line).map(String::as_str);
    match kind {
        CommandKind::Python => holds(ANY_PYTHON),
        CommandKind::Shell => holds(ANY_SHELL).or_else(|| {
            denies.iter().map(String::as_str).find(|line| {
                denied_head(line)
                    .is_some_and(|head| command_guard::names_shell_command(command, head))
            })
        }),
    }
}

pub fn card_rule(line: String, kind: CommandKind, command: &str) -> Option<String> {
    (GrantFile::AgentDeniedCommands.recognizes(&line)
        && matching(std::slice::from_ref(&line), kind, command).is_some())
    .then_some(line)
}

pub async fn live(pool: &sqlx::PgPool) -> Result<Vec<String>, BoxError> {
    tool_grants::standing_lines(pool, GrantFile::AgentDeniedCommands).await
}

pub async fn record(
    bus: &EventBus,
    thread_id: Uuid,
    pattern: String,
    by: AnsweredBy,
) -> Result<(), BoxError> {
    let changed_by = ChangedBy {
        source: GrantSource::Card,
        thread_id: Some(thread_id),
        actor: by.actor,
    };
    tool_grants::change_standing(
        bus,
        GrantFile::AgentDeniedCommands,
        StandingChange::Add(&[pattern]),
        changed_by,
    )
    .await
    .map(|_| ())
}

/// INVARIANT: Persist the line stamped by the card before releasing its blocked command.
pub async fn record_card_rule(
    engine: &crate::engine::LucidosEngine,
    entry: &crate::engine::cc_permission::PermissionEntry,
    by: &AnsweredBy,
) -> Result<String, BoxError> {
    let line = entry
        .grant_offer
        .deny
        .clone()
        .ok_or("This card offers no standing deny")?;
    record(&engine.event_bus, entry.thread_id, line.clone(), by.clone()).await?;
    Ok(line)
}

fn denied_words(line: &str) -> String {
    match (denied_head(line), line) {
        (Some(head), _) => format!("`{head}` commands"),
        (None, ANY_SHELL) => "every shell command".to_string(),
        (None, ANY_PYTHON) => "every Python run".to_string(),
        (None, other) => format!("`{other}`"),
    }
}

pub fn refusal(line: &str) -> String {
    format!(
        "Refused by a standing deny: the user set the command rule `{line}`, which always denies \
         {what}. The command was NOT run. Do not retry it, and do not reach the same program \
         another way. Only the user can remove the rule, in Settings > Permissions. Tell the user \
         which rule stopped you and ask how they want to proceed.",
        what = denied_words(line),
    )
}

pub fn unreadable_refusal(error: &dyn std::fmt::Display) -> String {
    format!(
        "Refused by the command guard: the standing denies could not be read ({error}), so the \
         command was NOT run. Try again later."
    )
}

pub async fn refusal_for(pool: &sqlx::PgPool, kind: CommandKind, command: &str) -> Option<String> {
    check_refusal(pool, kind, command)
        .await
        .unwrap_or_else(|error| Some(unreadable_refusal(&error)))
}

pub(in crate::engine) async fn check_refusal(
    pool: &sqlx::PgPool,
    kind: CommandKind,
    command: &str,
) -> Result<Option<String>, BoxError> {
    let denies = live(pool).await.inspect_err(|error| {
        crate::log_legacy!("[StandingDeny] Could not read the standing denies: {error}");
    })?;
    Ok(matching(&denies, kind, command).map(refusal))
}

#[cfg(test)]
#[path = "standing_deny_tests.rs"]
mod tests;
