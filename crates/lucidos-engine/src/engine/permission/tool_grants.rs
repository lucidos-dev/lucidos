//! *Tool grants*: the permission grants the three tool cards store, held as
//! `permissions` rows that carry their grant file line as `pattern`.
//!
//! The rows are the authority. Each lane's gate reads its lines here and
//! matches them with its own predicate, so a line means exactly what it meant
//! in the file. A standing grant belongs to the workspace, and "Allow for this
//! thread" is a row held by that thread.
//!
//! The grant file is a mirror the engine rewrites after every change, so a
//! downgrade reads exactly the live grants. A line added to the file by hand
//! grants nothing until the owner imports it ([`reconcile_mirror`]).
//!
//! Each lane imports its file once ([`import_locked`]). Until that import
//! commits, the gate reads the file as before, so a failed import never drops
//! a grant.

use std::collections::BTreeSet;
use std::path::Path;

use tokio::sync::Mutex;
use uuid::Uuid;

use super::{
    action_wire, Action, Holder, Lifetime, PermissionChoice, PickedBy, RequestCard, RequestOption,
};
use crate::core::grants::{self, GrantFile, GrantSource};
use crate::engine::claude_code::AllowScope;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Serializes each standing change with its mirror write, so the file always
/// matches the rows it was written from.
static STANDING_LOCK: Mutex<()> = Mutex::const_new(());

/// The unique index that lets each grant file be imported once.
const IMPORTED_UNIQUE_INDEX: &str = "events_grant_file_imported_unique";

/// The action a lane's grants cover.
pub fn lane_action(file: GrantFile) -> Action {
    match file {
        GrantFile::AgentCommands => Action::RunCommand,
        GrantFile::CodingAgentTools => Action::UseTool,
        GrantFile::McpTools => Action::UseMcpTool,
    }
}

/// The lane whose grants cover `action`, if it is a tool action.
pub fn lane_of(action: Action) -> Option<GrantFile> {
    GrantFile::ALL
        .into_iter()
        .find(|file| lane_action(*file) == action)
}

/// The lines a lane's gate matches for one thread.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GrantedLines {
    /// The workspace's standing grants.
    pub standing: Vec<String>,
    /// The thread's own "Allow for this thread" grants.
    pub thread: Vec<String>,
}

impl GrantedLines {
    /// Whether either set holds `pattern` exactly.
    pub fn contains(&self, pattern: &str) -> bool {
        self.standing
            .iter()
            .chain(&self.thread)
            .any(|p| p == pattern)
    }
}

/// The lines `file`'s gate matches for `thread_id`. Never an error: a read
/// that fails grants nothing, and asks.
///
/// A mirror that differs from the rows is reconciled first. So a line deleted
/// by hand stops granting at the next gate, as it did when the file decided.
pub async fn granted_lines(
    bus: &EventBus,
    dir: &Path,
    file: GrantFile,
    thread_id: Option<Uuid>,
) -> GrantedLines {
    // The file decides only while the lane is known not to be imported. An
    // import status that cannot be read grants nothing, so a hand-added line
    // never grants through a database error.
    match is_imported(bus.pool(), file).await {
        Ok(true) => {}
        Ok(false) => {
            let _guard = STANDING_LOCK.lock().await;
            if let Err(e) = import_locked(bus, dir, file).await {
                crate::log!(
                    "[ToolGrants] {} is not imported yet ({e}), so its gate reads the file",
                    file.file_name()
                );
                return GrantedLines {
                    standing: grants::patterns(dir, file),
                    thread: Vec::new(),
                };
            }
        }
        Err(e) => {
            crate::log!(
                "[ToolGrants] Cannot tell whether {} is imported ({e}). Treating as none.",
                file.file_name()
            );
            return GrantedLines::default();
        }
    }
    let read = |lines: Result<GrantedLines, BoxError>| {
        lines.unwrap_or_else(|e| {
            crate::log!(
                "[ToolGrants] Could not read the {} grants: {e}. Treating as none.",
                file.file_name()
            );
            GrantedLines::default()
        })
    };
    let lines = read(live_lines(bus.pool(), file, thread_id).await);
    let on_disk = std::fs::read_to_string(dir.join(file.file_name())).ok();
    if on_disk.as_deref() == Some(file.mirror_body(&lines.standing).as_str()) {
        return lines;
    }
    match reconcile_mirror(bus, dir, file).await {
        Ok(()) => read(live_lines(bus.pool(), file, thread_id).await),
        Err(e) => {
            crate::log!(
                "[ToolGrants] Could not reconcile {} ({e}), so only lines still in it grant",
                file.file_name()
            );
            still_in_file(lines, dir, file)
        }
    }
}

/// `lines` without any standing line the file no longer holds. A deleted line
/// is a revoke, which must hold even when recording it failed.
fn still_in_file(mut lines: GrantedLines, dir: &Path, file: GrantFile) -> GrantedLines {
    if let Ok(Some(on_disk)) = file_lines(dir, file) {
        lines.standing.retain(|l| on_disk.contains(l));
    }
    lines
}

async fn live_lines(
    pool: &sqlx::PgPool,
    file: GrantFile,
    thread_id: Option<Uuid>,
) -> Result<GrantedLines, BoxError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT holder_kind, pattern FROM permissions \
         WHERE action = $1 AND pattern IS NOT NULL AND ended_at IS NULL \
           AND (holder_kind = 'workspace' OR (holder_kind = 'thread' AND holder_id = $2)) \
         ORDER BY granted_at, pattern COLLATE \"C\"",
    )
    .bind(action_wire(lane_action(file))?)
    .bind(thread_id.map(|t| t.to_string()))
    .fetch_all(pool)
    .await?;
    let mut lines = GrantedLines::default();
    for (holder_kind, pattern) in rows {
        match holder_kind.as_str() {
            "workspace" => lines.standing.push(pattern),
            _ => lines.thread.push(pattern),
        }
    }
    Ok(lines)
}

/// The workspace's live standing lines for `file`, in the order the mirror
/// lists them.
pub async fn standing_lines(pool: &sqlx::PgPool, file: GrantFile) -> Result<Vec<String>, BoxError> {
    Ok(live_lines(pool, file, None).await?.standing)
}

async fn is_imported(pool: &sqlx::PgPool, file: GrantFile) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM events WHERE event_type = 'PermissionGrantsChanged' \
           AND payload->'data'->>'source' = 'legacy-import' \
           AND payload->'data'->>'grant_file' = $1)",
    )
    .bind(file.file_name())
    .fetch_one(pool)
    .await
}

/// Import `file` into the `permissions` table unless that already happened.
/// Callers hold [`STANDING_LOCK`]. One `PermissionGrantsChanged`
/// carries every line of the file, so the import lands whole or not at all,
/// and the event is the stamp.
async fn import_locked(bus: &EventBus, dir: &Path, file: GrantFile) -> Result<(), BoxError> {
    if is_imported(bus.pool(), file).await? {
        return Ok(());
    }
    let on_disk = file_lines(dir, file)?;
    let lines = dedup(on_disk.unwrap_or_else(|| {
        file.compiled_defaults()
            .iter()
            .map(|s| s.to_string())
            .collect()
    }));
    if file == GrantFile::CodingAgentTools {
        import_coding_agent_thread_allows(bus).await?;
    }
    let emitted = bus
        .emit(BusEvent::System(SystemEvent::PermissionGrantsChanged {
            grant_file: file,
            patterns: lines.clone(),
            source: GrantSource::LegacyImport,
            granted_in_thread_id: None,
            actor: None,
        }))
        .await;
    match emitted {
        Ok(_) => {}
        Err(e) if crate::engine::event_bus::violates_unique_index(&*e, IMPORTED_UNIQUE_INDEX) => {
            return Ok(());
        }
        Err(e) => return Err(e),
    }
    crate::log!(
        "[ToolGrants] Imported {} line(s) of {}",
        lines.len(),
        file.file_name()
    );
    write_mirror(dir, file, &lines);
    let unmapped: Vec<String> = lines.into_iter().filter(|l| !file.recognizes(l)).collect();
    if !unmapped.is_empty() {
        raise_review_card(bus, review_unmapped_card(file, unmapped)).await;
    }
    Ok(())
}

/// The coding-agent lane kept its "Allow for this thread" grants as resolution
/// events and rebuilt them per thread. Each becomes a thread-held row once.
///
/// `r.thread_id = q.thread_id` is a trust boundary: only events the engine wrote
/// on a thread carry one, so a forged domain event cannot become a grant. A
/// card already granted is skipped, so a second run adds nothing.
async fn import_coding_agent_thread_allows(bus: &EventBus) -> Result<(), BoxError> {
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT r.thread_id, r.payload->>'request_id', q.payload->'grant_offer'->>'session' \
         FROM events r \
         JOIN events q \
           ON q.event_type = 'CodingAgentPermissionRequest' \
          AND q.thread_id = r.thread_id \
          AND q.payload->>'request_id' = r.payload->>'request_id' \
         JOIN thread_summaries t ON t.thread_id = r.thread_id \
         WHERE r.event_type = 'CodingAgentPermissionResolved' \
           AND r.thread_id IS NOT NULL \
           AND (r.payload->>'allowed')::boolean IS TRUE \
           AND r.payload->>'persist_scope' = 'session' \
           AND q.payload->'grant_offer'->>'session' IS NOT NULL \
           AND NOT EXISTS (SELECT 1 FROM events g WHERE g.event_type = 'PermissionGranted' \
             AND g.thread_id = r.thread_id \
             AND g.payload->>'request_tool_use_id' = r.payload->>'request_id')",
    )
    .fetch_all(bus.pool())
    .await?;
    for (thread_id, request_id, pattern) in rows {
        // Older cards stamped patterns unscreened. One that is refused now is
        // left out, so it cannot block the import of every other grant.
        if refuse_control_characters(std::slice::from_ref(&pattern)).is_err() {
            crate::log!(
                "[ToolGrants] Left out a thread grant with a control character: {pattern:?}"
            );
            continue;
        }
        let by = AnsweredBy::tap(None);
        grant_for_thread(
            bus,
            GrantFile::CodingAgentTools,
            thread_id,
            &request_id,
            pattern,
            by,
        )
        .await?;
    }
    Ok(())
}

/// Who answered a tool card, and how they picked.
#[derive(Clone, Debug)]
pub struct AnsweredBy {
    pub actor: Option<MessageOrigin>,
    pub picked_by: PickedBy,
}

impl AnsweredBy {
    /// A tap on the card itself.
    pub fn tap(actor: Option<MessageOrigin>) -> Self {
        Self {
            actor,
            picked_by: PickedBy::Tap,
        }
    }
}

/// Record a tool card's "Allow for this thread": a row `thread_id` holds,
/// carrying `pattern`. `request_id` names the card.
pub async fn grant_for_thread(
    bus: &EventBus,
    file: GrantFile,
    thread_id: Uuid,
    request_id: &str,
    pattern: String,
    by: AnsweredBy,
) -> Result<(), BoxError> {
    refuse_control_characters(std::slice::from_ref(&pattern))?;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::PermissionGranted {
            permission_id: Uuid::new_v4(),
            action: lane_action(file),
            breadth: None,
            lifetime: Lifetime::ThisThread,
            holder: Holder::Thread { thread_id },
            request_tool_use_id: request_id.to_string(),
            picked_by: by.picked_by,
            pattern: Some(pattern),
        },
        meta: EventMeta::with_actor(by.actor),
    })
    .await?;
    Ok(())
}

/// Record a tool card's allow pick of `scope`, carrying `pattern`: a thread
/// grant for `Session`, else a standing line.
#[allow(clippy::too_many_arguments)]
pub async fn record_card_allow(
    bus: &EventBus,
    dir: &Path,
    file: GrantFile,
    thread_id: Uuid,
    request_id: &str,
    pattern: String,
    scope: AllowScope,
    by: AnsweredBy,
) -> Result<(), BoxError> {
    match scope {
        AllowScope::Session => {
            grant_for_thread(bus, file, thread_id, request_id, pattern, by).await
        }
        AllowScope::Narrow | AllowScope::Broad => {
            let changed_by = ChangedBy {
                source: GrantSource::Card,
                thread_id: Some(thread_id),
                actor: by.actor,
            };
            change_standing(bus, dir, file, StandingChange::Add(&[pattern]), changed_by)
                .await
                .map(|_| ())
        }
    }
}

/// One change to a lane's standing grants.
pub enum StandingChange<'a> {
    /// Grant these lines too.
    Add(&'a [String]),
    /// Revoke these lines.
    Remove(&'a [String]),
    /// The standing lines become exactly these.
    Replace(&'a [String]),
}

/// What made a standing change, as the audit row and new rows record it.
pub struct ChangedBy {
    pub source: GrantSource,
    pub thread_id: Option<Uuid>,
    pub actor: Option<MessageOrigin>,
}

/// Apply `change` to `file`'s standing grants, then rewrite the mirror.
/// `Ok(false)` when it changed nothing.
///
/// A hand edit is reconciled first, so the rewrite never undoes it. The mirror
/// is written before the event and restored if the event fails. A crash
/// between the two leaves a line the rows lack, which the next
/// [`reconcile_mirror`] reads as a hand edit. That way a crash never grants.
pub async fn change_standing(
    bus: &EventBus,
    dir: &Path,
    file: GrantFile,
    change: StandingChange<'_>,
    by: ChangedBy,
) -> Result<bool, BoxError> {
    let _guard = STANDING_LOCK.lock().await;
    let live = reconcile_locked(bus, dir, file).await?;
    let next = match change {
        StandingChange::Add(lines) => {
            refuse_control_characters(lines)?;
            with_added(&live, lines)
        }
        StandingChange::Remove(lines) => live
            .iter()
            .filter(|l| !lines.contains(l))
            .cloned()
            .collect(),
        StandingChange::Replace(lines) => {
            refuse_control_characters(lines)?;
            let kept: Vec<String> = live.iter().filter(|l| lines.contains(l)).cloned().collect();
            with_added(&kept, lines)
        }
    };
    if as_set(&next) == as_set(&live) {
        return Ok(false);
    }
    // A mirror lacking a line reads as a hand deletion, which revokes it. So
    // a change whose mirror cannot be written is not made at all.
    grants::write_raw(dir, file, &file.mirror_body(&next))?;
    if let Err(e) = emit_standing(bus, file, next, by).await {
        write_mirror(dir, file, &live);
        return Err(e);
    }
    Ok(true)
}

async fn emit_standing(
    bus: &EventBus,
    file: GrantFile,
    patterns: Vec<String>,
    by: ChangedBy,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::System(SystemEvent::PermissionGrantsChanged {
        grant_file: file,
        patterns,
        source: by.source,
        granted_in_thread_id: by.thread_id,
        actor: by.actor,
    }))
    .await?;
    Ok(())
}

/// `live`, then each of `lines` it lacks, sorted. New rows share one grant
/// time and the table orders them by line, so the mirror keeps that order.
fn with_added(live: &[String], lines: &[String]) -> Vec<String> {
    let added: BTreeSet<&String> = lines.iter().filter(|l| !live.contains(l)).collect();
    live.iter()
        .cloned()
        .chain(added.into_iter().cloned())
        .collect()
}

fn as_set(lines: &[String]) -> BTreeSet<&str> {
    lines.iter().map(String::as_str).collect()
}

fn dedup(lines: Vec<String>) -> Vec<String> {
    let mut seen = BTreeSet::new();
    lines
        .into_iter()
        .filter(|l| seen.insert(l.clone()))
        .collect()
}

/// The file is one grant per line, and a line derives from agent or MCP server
/// input. An embedded newline would write grants the owner never saw.
fn refuse_control_characters(lines: &[String]) -> Result<(), BoxError> {
    match lines.iter().find(|l| l.chars().any(char::is_control)) {
        Some(line) => {
            Err(format!("refusing a grant pattern with a control character: {line:?}").into())
        }
        None => Ok(()),
    }
}

/// The granted lines of `file`, or `None` when it is not on disk.
fn file_lines(dir: &Path, file: GrantFile) -> Result<Option<Vec<String>>, BoxError> {
    match std::fs::read_to_string(dir.join(file.file_name())) {
        Ok(body) => Ok(Some(grants::parse_patterns(&body))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("read {}: {e}", file.file_name()).into()),
    }
}

/// Rewrite the mirror. A failure costs only the downgrade copy and the
/// Settings view, never a decision, so it logs.
fn write_mirror(dir: &Path, file: GrantFile, lines: &[String]) {
    if let Err(e) = grants::write_raw(dir, file, &file.mirror_body(lines)) {
        crate::log!("[ToolGrants] Could not rewrite {}: {e}", file.file_name());
    }
}

/// Bring `file`'s mirror back in line with the rows, and act on a hand edit.
/// A deleted line is revoked, since deleting only takes power away. An added
/// line grants nothing: it goes on a card in the home thread, where the owner
/// imports it or not. Runs at boot, before every standing change, and at any
/// gate that finds the file differs from the rows.
pub async fn reconcile_mirror(bus: &EventBus, dir: &Path, file: GrantFile) -> Result<(), BoxError> {
    reconciled_standing(bus, dir, file).await.map(|_| ())
}

/// The standing lines once the mirror is reconciled. For a reader that must
/// never mistake a failed read for no grants, as a gate may.
pub async fn reconciled_standing(
    bus: &EventBus,
    dir: &Path,
    file: GrantFile,
) -> Result<Vec<String>, BoxError> {
    let _guard = STANDING_LOCK.lock().await;
    reconcile_locked(bus, dir, file).await
}

/// [`reconcile_mirror`] under [`STANDING_LOCK`]. Returns the standing lines it
/// kept.
async fn reconcile_locked(
    bus: &EventBus,
    dir: &Path,
    file: GrantFile,
) -> Result<Vec<String>, BoxError> {
    import_locked(bus, dir, file).await?;
    let live = standing_lines(bus.pool(), file).await?;
    let Some(on_disk) = file_lines(dir, file)? else {
        write_mirror(dir, file, &live);
        return Ok(live);
    };
    let added: Vec<String> = dedup(
        on_disk
            .iter()
            .filter(|l| !live.contains(l))
            .cloned()
            .collect(),
    );
    let kept: Vec<String> = live
        .iter()
        .filter(|l| on_disk.contains(l))
        .cloned()
        .collect();
    if kept.len() < live.len() {
        crate::log!(
            "[ToolGrants] {} line(s) were deleted from {} by hand, so they are revoked",
            live.len() - kept.len(),
            file.file_name()
        );
        let by = ChangedBy {
            source: GrantSource::FileEdit,
            thread_id: None,
            actor: None,
        };
        emit_standing(bus, file, kept.clone(), by).await?;
    }
    // The added lines leave the file before their card is raised. A write that
    // fails keeps them there, and the next reconcile raises the card once.
    let body = file.mirror_body(&kept);
    let current = std::fs::read_to_string(dir.join(file.file_name())).ok();
    if current.as_deref() != Some(body.as_str()) {
        grants::write_raw(dir, file, &body)?;
    }
    if !added.is_empty() {
        crate::log!(
            "[ToolGrants] {} line(s) were added to {} by hand. They grant nothing until imported.",
            added.len(),
            file.file_name()
        );
        raise_review_card(bus, review_added_card(file, added)).await;
    }
    Ok(kept)
}

/// Reconcile every lane's mirror. A lane that fails logs and leaves the rest.
pub async fn reconcile_all_mirrors(bus: &EventBus, dir: &Path) {
    for file in GrantFile::ALL {
        if let Err(e) = reconcile_mirror(bus, dir, file).await {
            crate::log!("[ToolGrants] Could not reconcile {}: {e}", file.file_name());
        }
    }
}

/// The grant file lines a review card is about.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GrantLines {
    pub grant_file: GrantFile,
    pub patterns: Vec<String>,
}

fn listed(lines: &[String]) -> String {
    lines
        .iter()
        .map(|l| format!("- {}", code_span(l)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` as one markdown code span. A hand-added line is untrusted. Its fence
/// outruns every backtick run inside it, so the line cannot close the span and
/// write prose of its own onto the card.
fn code_span(text: &str) -> String {
    let longest_run = text.split(|c| c != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest_run + 1);
    // CommonMark strips one space from each side, so a backtick at an edge
    // stays content rather than joining the fence.
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

/// The card for lines added to a grant file by hand.
fn review_added_card(file: GrantFile, patterns: Vec<String>) -> (RequestCard, GrantLines) {
    let card = RequestCard {
        question: format!(
            "These lines were added to `{}` outside Lucidos. They grant nothing until you \
             import them.\n{}",
            file.file_name(),
            listed(&patterns)
        ),
        header: "Grants",
        action: lane_action(file),
        breadth: None,
        options: vec![
            RequestOption {
                label: "Import".into(),
                description: "Grant every listed line, until you revoke it.".into(),
                choice: PermissionChoice::grant(Lifetime::Standing, Holder::Workspace),
            },
            RequestOption {
                label: "Ignore".into(),
                description: "Grant none of them. The file keeps only what Lucidos holds.".into(),
                choice: PermissionChoice::Refuse,
            },
        ],
        batch: None,
        unattested_act: None,
    };
    (
        card,
        GrantLines {
            grant_file: file,
            patterns,
        },
    )
}

/// The card for imported lines whose shape the lane does not read.
fn review_unmapped_card(file: GrantFile, patterns: Vec<String>) -> (RequestCard, GrantLines) {
    let card = RequestCard {
        question: format!(
            "Lucidos cannot read the shape of these lines in `{}`. Each still grants exactly \
             what it granted before.\n{}",
            file.file_name(),
            listed(&patterns)
        ),
        header: "Grants",
        action: lane_action(file),
        breadth: None,
        options: vec![
            RequestOption {
                label: "Keep".into(),
                description: "They keep granting what they granted before.".into(),
                choice: PermissionChoice::Refuse,
            },
            RequestOption {
                label: "Revoke".into(),
                description: "They stop granting anything.".into(),
                choice: PermissionChoice::Revoke,
            },
        ],
        batch: None,
        unattested_act: None,
    };
    (
        card,
        GrantLines {
            grant_file: file,
            patterns,
        },
    )
}

/// Raise a review card in the home thread. A failure logs: the lines grant
/// nothing either way.
async fn raise_review_card(bus: &EventBus, (card, lines): (RequestCard, GrantLines)) {
    let raised = match crate::engine::home_thread::ensure_home_thread(bus, bus.pool()).await {
        Ok(home) => {
            super::raise_request_about(bus, home, card, Some(lines), EventChannel::Chat).await
        }
        Err(e) => Err(e),
    };
    if let Err(e) = raised {
        crate::log!("[ToolGrants] Could not raise the grant review card: {e}");
    }
}

/// Act on a pick on a review card, and say whether the grants changed. Only
/// the owner's own pick imports lines, since a batch never picks a standing
/// grant.
pub(super) async fn act_on_review(
    bus: &EventBus,
    dir: &Path,
    thread_id: Uuid,
    lines: &GrantLines,
    choice: &PermissionChoice,
    actor: Option<&MessageOrigin>,
    picked_by: &PickedBy,
) -> Result<bool, BoxError> {
    let (change, source) = match choice {
        PermissionChoice::Grant { .. } if matches!(picked_by, PickedBy::Batch { .. }) => {
            crate::log!("[ToolGrants] A batch picked Import on a review card. It imports nothing.");
            return Ok(false);
        }
        PermissionChoice::Grant { .. } => (
            StandingChange::Add(&lines.patterns),
            GrantSource::FileReview,
        ),
        PermissionChoice::Revoke => (StandingChange::Remove(&lines.patterns), GrantSource::Revoke),
        PermissionChoice::Unpause | PermissionChoice::Refuse => return Ok(false),
    };
    let by = ChangedBy {
        source,
        thread_id: Some(thread_id),
        actor: actor.cloned(),
    };
    change_standing(bus, dir, lines.grant_file, change, by).await
}

#[cfg(test)]
#[path = "tool_grants_tests.rs"]
mod tests;
