use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::core::changes::{Change, ChangeStatus, RestartGroup};
use crate::engine::thread_lifecycle::ChangeStateKind;

const SELECT_CHANGE: &str =
    "SELECT id, request_id, thread_id, branch_name, repo_root, description, \
     file_count, files, requires_restart, status, created_at, resolved_at, \
     merge_worktree_path, merge_temp_branch, hardened, pre_merge_sha, \
     post_merge_sha, NULL::text AS thread_title, commits, summary, incomplete FROM changes";

const MERGE_PAIRING_EVENT_TYPES: &str = "'MergeConflictDetected','MergeResolutionCleared',\
     'ChangeApplyFailed','ChangeApplied','ChangeDiscarded'";

pub async fn open_conflict_pairings(
    pool: &PgPool,
    pairs: &[(Uuid, Uuid)],
) -> sqlx::Result<std::collections::HashMap<Uuid, DateTime<Utc>>> {
    if pairs.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let thread_ids: Vec<String> = pairs.iter().map(|(t, _)| t.to_string()).collect();
    let change_ids: Vec<String> = pairs.iter().map(|(_, c)| c.to_string()).collect();
    let open: Vec<(String, DateTime<Utc>)> = sqlx::query_as(&format!(
        "SELECT change_id, created FROM ( \
            SELECT DISTINCT ON (payload->>'change_id') \
                   payload->>'change_id' AS change_id, event_type, created \
            FROM events \
            WHERE aggregate_id = ANY($1) \
              AND payload->>'change_id' = ANY($2) \
              AND event_type IN ({MERGE_PAIRING_EVENT_TYPES}) \
            ORDER BY payload->>'change_id', sequence DESC \
         ) latest \
         WHERE event_type = 'MergeConflictDetected'"
    ))
    .bind(&thread_ids)
    .bind(&change_ids)
    .fetch_all(pool)
    .await?;
    Ok(open
        .into_iter()
        .filter_map(|(id, at)| Uuid::parse_str(&id).ok().map(|id| (id, at)))
        .collect())
}

#[derive(Clone)]
pub struct ChangesProjection {
    pool: PgPool,
}

fn parse_change_id(change_id: &str) -> Option<Uuid> {
    Uuid::parse_str(change_id).ok()
}

impl ChangesProjection {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// INVARIANT: Recompute a thread's change state and `coding_agent_requires_restart`
    /// from its pending changes. `proposed` is a cache of this table, and
    /// nothing else writes it. Every write below calls this in its transaction
    /// when it moves a change in or out of `pending`, or flips a restart flag.
    ///
    /// `leaving` is where a thread lands when its last pending change goes:
    /// [`landing_after`] of the status that took it.
    pub(crate) async fn sync_thread_proposal<'e, E>(
        executor: E,
        thread_id: Uuid,
        leaving: ChangeStateKind,
    ) -> sqlx::Result<()>
    where
        E: sqlx::Executor<'e, Database = Postgres>,
    {
        sqlx::query(&format!(
            "WITH p AS ( \
                SELECT COUNT(*) > 0 AS pending, \
                       COALESCE(bool_or(requires_restart), FALSE) AS requires_restart \
                FROM changes WHERE thread_id = $1 AND status = $2), \
             n AS ( \
                SELECT CASE WHEN p.pending THEN '{proposed}' \
                            WHEN t.coding_agent_change_state = '{proposed}' THEN $3 \
                            ELSE t.coding_agent_change_state END AS state, \
                       CASE WHEN p.pending OR t.coding_agent_change_state = '{proposed}' \
                            THEN NULL ELSE t.coding_agent_unproposed_reason END AS reason, \
                       p.pending AND p.requires_restart AS requires_restart \
                FROM thread_summaries t, p WHERE t.thread_id = $1) \
             UPDATE thread_summaries t SET \
                coding_agent_change_state = n.state, \
                coding_agent_unproposed_reason = n.reason, \
                coding_agent_requires_restart = n.requires_restart \
             FROM n \
             WHERE t.thread_id = $1 \
               AND (t.coding_agent_change_state, t.coding_agent_unproposed_reason, \
                    t.coding_agent_requires_restart) \
                   IS DISTINCT FROM (n.state, n.reason, n.requires_restart)",
            proposed = ChangeStateKind::Proposed.as_str(),
        ))
        .bind(thread_id)
        .bind(ChangeStatus::Pending)
        .bind(leaving.as_str())
        .execute(executor)
        .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn write_proposed_aggregate(
        tx: &mut Transaction<'_, Postgres>,
        change_id: &str,
        thread_id: Uuid,
        branch_name: &str,
        repo_root: &str,
        description: Option<&str>,
        files: &[String],
        requires_restart: bool,
        hardened: bool,
        incomplete: bool,
        status: ChangeStatus,
    ) -> sqlx::Result<()> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(());
        };
        // INVARIANT: description bound as Option<&str>: COALESCE on insert defaults to ''
        // (column is NOT NULL); COALESCE on update preserves the existing
        // description when the re-emit carries None — matches the in-memory
        // contract `if let Some(d) = description { existing.description = d }`.
        // `incomplete` propagates verbatim on re-emit so a follow-up
        // successful turn against the same branch clears a prior failure tag.
        sqlx::query(
            "INSERT INTO changes (id, request_id, thread_id, branch_name, repo_root, \
             description, file_count, files, requires_restart, status, created_at, hardened, incomplete) \
             VALUES ($1, $2, $3, $4, $5, COALESCE($6, ''), $7, $8, $9, $12, NOW(), $10, $11) \
             ON CONFLICT (id) DO UPDATE SET \
                summary = CASE WHEN COALESCE($6, changes.description) = changes.description \
                          THEN changes.summary END, \
                description = COALESCE($6, changes.description), \
                files = EXCLUDED.files, \
                file_count = EXCLUDED.file_count, \
                requires_restart = EXCLUDED.requires_restart, \
                hardened = EXCLUDED.hardened, \
                incomplete = EXCLUDED.incomplete",
        )
        .bind(id)
        .bind(Uuid::nil())
        .bind(thread_id)
        .bind(branch_name)
        .bind(repo_root)
        .bind(description)
        .bind(files.len() as i32)
        .bind(files)
        .bind(requires_restart)
        .bind(hardened)
        .bind(incomplete)
        .bind(status)
        .execute(&mut **tx)
        .await?;
        Self::sync_thread_proposal(&mut **tx, thread_id, landing_after(status)).await
    }

    pub(crate) async fn write_proposed_per_commit(
        tx: &mut Transaction<'_, Postgres>,
        branch_name: &str,
        description: Option<&str>,
        requires_restart: bool,
    ) -> sqlx::Result<()> {
        let thread_ids: Vec<Option<Uuid>> = sqlx::query_scalar(
            "UPDATE changes SET \
             summary = CASE WHEN COALESCE($2, description) = description THEN summary END, \
             description = COALESCE($2, description), \
             requires_restart = requires_restart OR $3, \
             hardened = FALSE \
             WHERE branch_name = $1 AND status = $4 \
             RETURNING thread_id",
        )
        .bind(branch_name)
        .bind(description)
        .bind(requires_restart)
        .bind(ChangeStatus::Pending)
        .fetch_all(&mut **tx)
        .await?;
        for thread_id in thread_ids.into_iter().flatten() {
            Self::sync_thread_proposal(&mut **tx, thread_id, ChangeStateKind::Unproposed).await?;
        }
        Ok(())
    }

    pub(crate) async fn write_applied(
        tx: &mut Transaction<'_, Postgres>,
        change_id: &str,
        requires_restart: bool,
        commits: &[String],
        pre_merge_sha: Option<&str>,
        post_merge_sha: Option<&str>,
    ) -> sqlx::Result<()> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(());
        };
        let row: Option<Option<Uuid>> = sqlx::query_scalar(
            "UPDATE changes SET status = $6, resolved_at = NOW(), \
             requires_restart = $2, \
             commits = CASE WHEN cardinality($3::text[]) > 0 THEN $3 ELSE commits END, \
             pre_merge_sha = COALESCE($4, pre_merge_sha), \
             post_merge_sha = COALESCE($5, post_merge_sha) \
             WHERE id = $1 \
             RETURNING thread_id",
        )
        .bind(id)
        .bind(requires_restart)
        .bind(commits)
        .bind(pre_merge_sha)
        .bind(post_merge_sha)
        .bind(ChangeStatus::Applied)
        .fetch_optional(&mut **tx)
        .await?;
        match row {
            None => {
                crate::log_legacy!("[ChangesProjection] write_applied: no row for {}: missing aggregate ChangeProposed?", id);
            }
            Some(Some(thread_id)) => {
                Self::sync_thread_proposal(
                    &mut **tx,
                    thread_id,
                    landing_after(ChangeStatus::Applied),
                )
                .await?
            }
            Some(None) => {}
        }
        Ok(())
    }

    pub(crate) async fn write_status(
        tx: &mut Transaction<'_, Postgres>,
        change_id: &str,
        status: ChangeStatus,
    ) -> sqlx::Result<()> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(());
        };
        let row: Option<Option<Uuid>> = sqlx::query_scalar(
            "UPDATE changes SET status = $2, \
             resolved_at = CASE WHEN $2 IN ($3, $4) THEN NULL ELSE NOW() END \
             WHERE id = $1 RETURNING thread_id",
        )
        .bind(id)
        .bind(status)
        .bind(ChangeStatus::Pending)
        .bind(ChangeStatus::SetAside)
        .fetch_optional(&mut **tx)
        .await?;
        match row {
            None => {
                crate::log_legacy!("[ChangesProjection] write_status({}) for {}: no row, missing aggregate ChangeProposed?", status, id);
            }
            Some(Some(thread_id)) => {
                Self::sync_thread_proposal(&mut **tx, thread_id, landing_after(status)).await?
            }
            Some(None) => {}
        }
        Ok(())
    }

    pub(crate) async fn write_hardened(
        tx: &mut Transaction<'_, Postgres>,
        change_id: &str,
    ) -> sqlx::Result<()> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(());
        };
        sqlx::query("UPDATE changes SET hardened = TRUE WHERE id = $1")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    pub(crate) async fn write_summary(
        tx: &mut Transaction<'_, Postgres>,
        change_id: &str,
        summary: &str,
        description: &str,
    ) -> sqlx::Result<()> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(());
        };
        sqlx::query("UPDATE changes SET summary = $2 WHERE id = $1 AND description = $3")
            .bind(id)
            .bind(summary)
            .bind(description)
            .execute(&mut **tx)
            .await?;
        Ok(())
    }

    pub(crate) async fn write_merge_started(
        tx: &mut Transaction<'_, Postgres>,
        change_id: &str,
        worktree_path: &str,
        temp_branch: &str,
    ) -> sqlx::Result<()> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(());
        };
        sqlx::query(
            "UPDATE changes SET merge_worktree_path = $2, merge_temp_branch = $3 \
             WHERE id = $1",
        )
        .bind(id)
        .bind(worktree_path)
        .bind(temp_branch)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub(crate) async fn write_merge_cleared(
        tx: &mut Transaction<'_, Postgres>,
        change_id: &str,
    ) -> sqlx::Result<()> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(());
        };
        sqlx::query(
            "UPDATE changes SET merge_worktree_path = NULL, merge_temp_branch = NULL \
             WHERE id = $1",
        )
        .bind(id)
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    pub async fn list_pending(&self) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status = $1 ORDER BY created_at ASC"
        ))
        .bind(ChangeStatus::Pending)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_by_id(&self, id: Uuid) -> sqlx::Result<Option<Change>> {
        sqlx::query_as(&format!("{SELECT_CHANGE} WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    pub async fn list_set_aside(&self) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status = $1 ORDER BY created_at DESC"
        ))
        .bind(ChangeStatus::SetAside)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn get_pending_by_branch(&self, branch_name: &str) -> sqlx::Result<Option<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE branch_name = $1 AND status = $2 LIMIT 1"
        ))
        .bind(branch_name)
        .bind(ChangeStatus::Pending)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn get_open_by_branch(&self, branch_name: &str) -> sqlx::Result<Option<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE branch_name = $1 AND status IN ($2, $3) \
             ORDER BY status = $2 DESC LIMIT 1"
        ))
        .bind(branch_name)
        .bind(ChangeStatus::Pending)
        .bind(ChangeStatus::SetAside)
        .fetch_optional(&self.pool)
        .await
    }

    pub async fn branch_has_decided_change(&self, branch_name: &str) -> sqlx::Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM changes WHERE branch_name = $1 AND status <> $2)",
        )
        .bind(branch_name)
        .bind(ChangeStatus::Withdrawn)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn branch_has_decided_change_since_handoff(
        &self,
        branch_name: &str,
        thread_id: Uuid,
    ) -> sqlx::Result<bool> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM changes WHERE branch_name = $1 AND status <> $2 \
             AND (status IN ($3, $4) OR created_at > (SELECT MAX(created) FROM events \
                 WHERE thread_id = $5 AND event_type = 'SuccessorThreadSpawned')))",
        )
        .bind(branch_name)
        .bind(ChangeStatus::Withdrawn)
        .bind(ChangeStatus::Pending)
        .bind(ChangeStatus::SetAside)
        .bind(thread_id)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn pending_for_thread(&self, thread_id: Uuid) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status = $2 AND thread_id = $1 ORDER BY created_at ASC"
        ))
        .bind(thread_id)
        .bind(ChangeStatus::Pending)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn open_for_thread(&self, thread_id: Uuid) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status IN ($2, $3) AND thread_id = $1 ORDER BY created_at ASC"
        ))
        .bind(thread_id)
        .bind(ChangeStatus::Pending)
        .bind(ChangeStatus::SetAside)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn pending_conflict_change_for_thread(
        &self,
        thread_id: Uuid,
    ) -> sqlx::Result<Option<Change>> {
        let open_ids: Vec<String> = sqlx::query_scalar(&format!(
            "SELECT change_id FROM ( \
                SELECT DISTINCT ON (payload->>'change_id') \
                       payload->>'change_id' AS change_id, event_type, sequence \
                FROM events \
                WHERE aggregate_id = $1 \
                  AND payload->>'change_id' IS NOT NULL \
                  AND event_type IN ({MERGE_PAIRING_EVENT_TYPES}) \
                ORDER BY payload->>'change_id', sequence DESC \
             ) latest \
             WHERE event_type = 'MergeConflictDetected' \
             ORDER BY sequence DESC"
        ))
        .bind(thread_id.to_string())
        .fetch_all(&self.pool)
        .await?;
        for id in open_ids {
            let Ok(change_id) = Uuid::parse_str(&id) else {
                continue;
            };
            if let Some(change) = self.get_by_id(change_id).await? {
                if change.is_pending() {
                    return Ok(Some(change));
                }
            }
        }
        Ok(None)
    }

    pub async fn conflict_pairing_open(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
    ) -> sqlx::Result<bool> {
        let open = open_conflict_pairings(&self.pool, &[(thread_id, change_id)]).await?;
        if !open.contains_key(&change_id) {
            return Ok(false);
        }
        Ok(self
            .get_by_id(change_id)
            .await?
            .is_some_and(|c| c.is_pending()))
    }

    pub async fn other_open_for_branch(
        &self,
        branch_name: &str,
        exclude_id: Uuid,
    ) -> sqlx::Result<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM changes \
             WHERE status IN ($3, $4) AND branch_name = $1 AND id <> $2)",
        )
        .bind(branch_name)
        .bind(exclude_id)
        .bind(ChangeStatus::Pending)
        .bind(ChangeStatus::SetAside)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn list_recently_applied(
        &self,
        limit: i64,
        before: Option<DateTime<Utc>>,
    ) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status IN ($3, $4) \
             AND ($2::timestamptz IS NULL OR resolved_at < $2) \
             ORDER BY resolved_at DESC NULLS LAST LIMIT $1"
        ))
        .bind(limit.max(0))
        .bind(before)
        .bind(ChangeStatus::Applied)
        .bind(ChangeStatus::Reverted)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn list_for_repo(
        &self,
        repo_root: &str,
        applied_limit: i64,
        before: Option<DateTime<Utc>>,
    ) -> sqlx::Result<(Vec<Change>, Vec<Change>, bool)> {
        let fetch_limit = applied_limit.max(0) + 1;
        let pending_sql = format!(
            "{SELECT_CHANGE} WHERE status = $2 AND repo_root = $1 \
             ORDER BY created_at ASC"
        );
        let applied_sql = format!(
            "{SELECT_CHANGE} WHERE status IN ($4, $5) AND repo_root = $1 \
             AND ($3::timestamptz IS NULL OR resolved_at < $3) \
             ORDER BY resolved_at DESC NULLS LAST LIMIT $2"
        );
        let (pending_r, applied_r) = tokio::join!(
            sqlx::query_as(&pending_sql)
                .bind(repo_root)
                .bind(ChangeStatus::Pending)
                .fetch_all(&self.pool),
            sqlx::query_as(&applied_sql)
                .bind(repo_root)
                .bind(fetch_limit)
                .bind(before)
                .bind(ChangeStatus::Applied)
                .bind(ChangeStatus::Reverted)
                .fetch_all(&self.pool),
        );

        let pending: Vec<Change> = pending_r?;
        let mut applied: Vec<Change> = applied_r?;

        let has_more = applied.len() as i64 > applied_limit;
        if has_more {
            applied.truncate(applied_limit.max(0) as usize);
        }
        Ok((pending, applied, has_more))
    }

    pub async fn requires_restart_since(&self, since: DateTime<Utc>) -> sqlx::Result<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM changes \
             WHERE status = $2 AND requires_restart AND resolved_at > $1)",
        )
        .bind(since)
        .bind(ChangeStatus::Applied)
        .fetch_one(&self.pool)
        .await
    }

    pub async fn client_update_since(&self, since: DateTime<Utc>) -> sqlx::Result<bool> {
        let files: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT f FROM changes c, unnest(c.files) AS f \
             WHERE c.status = $2 AND c.resolved_at > $1",
        )
        .bind(since)
        .bind(ChangeStatus::Applied)
        .fetch_all(&self.pool)
        .await?;
        Ok(super::git::files_have_client_update(&files))
    }

    pub async fn restart_groups_since(
        &self,
        since: DateTime<Utc>,
    ) -> sqlx::Result<Vec<RestartGroup>> {
        let rows: Vec<(Option<Uuid>, Vec<String>)> = sqlx::query_as(
            "SELECT thread_id, commits FROM changes \
             WHERE status = $2 AND requires_restart AND resolved_at > $1 \
             ORDER BY resolved_at ASC",
        )
        .bind(since)
        .bind(ChangeStatus::Applied)
        .fetch_all(&self.pool)
        .await?;

        let mut groups: Vec<RestartGroup> = Vec::new();
        for (thread_id, commits) in rows {
            if let Some(g) = groups.iter_mut().find(|g| g.thread_id == thread_id) {
                for commit in &commits {
                    if !g.commits.contains(commit) {
                        g.commits.push(commit.clone());
                    }
                }
            } else {
                groups.push(RestartGroup {
                    thread_id,
                    thread_title: None,
                    commits,
                });
            }
        }
        Ok(groups)
    }

    pub async fn with_merge_worktree(&self) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status = $1 AND merge_worktree_path IS NOT NULL"
        ))
        .bind(ChangeStatus::Pending)
        .fetch_all(&self.pool)
        .await
    }

    pub async fn rebuild_missing_from_events(&self) -> sqlx::Result<usize> {
        let missing_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT (payload->>'change_id')::uuid AS change_id
             FROM events
             WHERE event_type IN (
                 'ChangeProposed','ChangeApplied','ChangeDiscarded',
                 'ChangeReverted','ChangeHardened','ChangeSummarized',
                 'ChangeSetAside','ChangeBroughtBack','ChangeWithdrawn',
                 'MergeResolutionStarted','MergeResolutionCleared'
             )
               AND payload->>'change_id' IS NOT NULL
               AND payload->>'change_id' != ''
               AND payload->>'change_id' ~ '^[0-9a-f-]{36}$'
               AND (payload->>'change_id')::uuid NOT IN (SELECT id FROM changes)",
        )
        .fetch_all(&self.pool)
        .await?;

        if missing_ids.is_empty() {
            return Ok(0);
        }

        let mut recovered = 0usize;
        for id in &missing_ids {
            if rebuild_one_from_events(&self.pool, *id).await? {
                recovered += 1;
            }
        }
        crate::log_legacy!(
            "[ChangesProjection] rebuild_missing_from_events: recovered {}/{} change rows",
            recovered,
            missing_ids.len()
        );
        Ok(recovered)
    }
}

async fn rebuild_one_from_events(pool: &PgPool, change_id: Uuid) -> sqlx::Result<bool> {
    let rows: Vec<(String, serde_json::Value, Option<Uuid>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT event_type, payload, thread_id, created
         FROM events
         WHERE payload->>'change_id' = $1
           AND event_type IN (
               'ChangeProposed','ChangeApplied','ChangeDiscarded',
               'ChangeReverted','ChangeHardened','ChangeSummarized',
               'ChangeSetAside','ChangeBroughtBack','ChangeWithdrawn',
               'MergeResolutionStarted','MergeResolutionCleared'
           )
         ORDER BY created",
    )
    .bind(change_id.to_string())
    .fetch_all(pool)
    .await?;

    // INVARIANT: Per-commit ChangeProposed emits carry an empty change_id (filtered out
    // upstream by the uuid regex), so any ChangeProposed reaching us here is
    // an aggregate. The `commit_sha` guard is defense in depth.
    let is_aggregate_proposed = |r: &(String, serde_json::Value, Option<Uuid>, DateTime<Utc>)| {
        r.0 == "ChangeProposed" && r.1.get("commit_sha").map(|v| v.is_null()).unwrap_or(true)
    };

    let Some(first) = rows.iter().find(|r| is_aggregate_proposed(r)) else {
        crate::log_legacy!(
            "[ChangesProjection] rebuild_one: change {} has no aggregate ChangeProposed — skipping",
            change_id
        );
        return Ok(false);
    };
    let latest = rows
        .iter()
        .rev()
        .find(|r| is_aggregate_proposed(r))
        .expect("forward find above guarantees at least one aggregate ChangeProposed row");

    let str_field = |p: &serde_json::Value, k: &str| {
        p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
    };
    let bool_field =
        |p: &serde_json::Value, k: &str| p.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    let str_array = |p: &serde_json::Value, k: &str| -> Vec<String> {
        p.get(k)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };

    let branch_name = str_field(&latest.1, "branch_name");
    let repo_root = str_field(&latest.1, "repo_root");
    let description = str_field(&latest.1, "description");
    let files = str_array(&latest.1, "files");
    let mut hardened = bool_field(&latest.1, "hardened");
    let mut requires_restart = bool_field(&latest.1, "requires_restart");
    let incomplete = bool_field(&latest.1, "incomplete");

    for r in &rows {
        if r.0 == "ChangeHardened" && r.3 > latest.3 {
            hardened = true;
        }
    }
    let summary = rows
        .iter()
        .rev()
        .filter(|r| r.0 == "ChangeSummarized")
        .find(|r| str_field(&r.1, "description") == description)
        .map(|r| str_field(&r.1, "summary"))
        .filter(|s| !s.is_empty());

    let proposed_set_aside = |r: &(String, serde_json::Value, Option<Uuid>, DateTime<Utc>)| {
        is_aggregate_proposed(r) && bool_field(&r.1, "set_aside")
    };
    let terminal = rows
        .iter()
        .rev()
        .find(|r| {
            proposed_set_aside(r)
                || matches!(
                    r.0.as_str(),
                    "ChangeApplied"
                        | "ChangeDiscarded"
                        | "ChangeReverted"
                        | "ChangeSetAside"
                        | "ChangeBroughtBack"
                        | "ChangeWithdrawn"
                )
        })
        .filter(|r| r.0 != "ChangeBroughtBack");

    // INVARIANT: Active merge worktree state: the latest unpaired MergeResolutionStarted.
    // Without this, a recovered change with a half-finished merge is invisible
    // to with_merge_worktree() and the on-disk worktree leaks.
    let (merge_worktree_path, merge_temp_branch) = {
        let mut wt: Option<&str> = None;
        let mut tb: Option<&str> = None;
        for r in &rows {
            match r.0.as_str() {
                "MergeResolutionStarted" => {
                    wt = r.1.get("worktree_path").and_then(|v| v.as_str());
                    tb = r.1.get("temp_branch").and_then(|v| v.as_str());
                }
                "MergeResolutionCleared" => {
                    wt = None;
                    tb = None;
                }
                _ => {}
            }
        }
        (wt, tb)
    };

    let (status, resolved_at, commits, pre_sha, post_sha) = match terminal {
        Some(t) if t.0 == "ChangeSetAside" || t.0 == "ChangeProposed" => {
            (ChangeStatus::SetAside, None, Vec::new(), None, None)
        }
        Some(t) => {
            let status = match t.0.as_str() {
                "ChangeApplied" => ChangeStatus::Applied,
                "ChangeDiscarded" => ChangeStatus::Discarded,
                "ChangeReverted" => ChangeStatus::Reverted,
                "ChangeWithdrawn" => ChangeStatus::Withdrawn,
                _ => unreachable!(),
            };
            if t.0 == "ChangeApplied" {
                requires_restart = bool_field(&t.1, "requires_restart") || requires_restart;
            }
            let commits = str_array(&t.1, "commits");
            let pre =
                t.1.get("pre_merge_sha")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            let post =
                t.1.get("post_merge_sha")
                    .and_then(|v| v.as_str())
                    .map(String::from);
            (status, Some(t.3), commits, pre, post)
        }
        None => (ChangeStatus::Pending, None, Vec::new(), None, None),
    };

    let res = sqlx::query(
        "INSERT INTO changes (id, request_id, thread_id, branch_name, repo_root,
         description, file_count, files, requires_restart, status, created_at,
         resolved_at, hardened, commits, pre_merge_sha, post_merge_sha,
         merge_worktree_path, merge_temp_branch, incomplete, summary)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(change_id)
    .bind(Uuid::nil())
    .bind(first.2)
    .bind(&branch_name)
    .bind(&repo_root)
    .bind(&description)
    .bind(files.len() as i32)
    .bind(&files)
    .bind(requires_restart)
    .bind(status)
    .bind(first.3)
    .bind(resolved_at)
    .bind(hardened)
    .bind(&commits)
    .bind(pre_sha.as_deref())
    .bind(post_sha.as_deref())
    .bind(merge_worktree_path)
    .bind(merge_temp_branch)
    .bind(incomplete)
    .bind(summary)
    .execute(pool)
    .await?;

    if res.rows_affected() == 0 {
        return Ok(false);
    }
    if let Some(thread_id) = first.2 {
        ChangesProjection::sync_thread_proposal(pool, thread_id, landing_after(status)).await?;
    }
    Ok(true)
}

pub(crate) fn landing_after(status: ChangeStatus) -> ChangeStateKind {
    match status {
        ChangeStatus::Applied | ChangeStatus::Discarded | ChangeStatus::Reverted => {
            ChangeStateKind::None
        }
        ChangeStatus::Pending | ChangeStatus::SetAside | ChangeStatus::Withdrawn => {
            ChangeStateKind::Unproposed
        }
    }
}

#[cfg(test)]
#[path = "changes_projection_tests/helpers.rs"]
mod cp_helpers;

#[cfg(test)]
#[path = "changes_projection_tests/projection.rs"]
mod projection_tests;

#[cfg(test)]
#[path = "changes_projection_tests/rebuild.rs"]
mod rebuild_tests;

#[cfg(test)]
#[path = "changes_projection_tests/set_aside.rs"]
mod set_aside_tests;

#[cfg(test)]
#[path = "changes_projection_tests/summary.rs"]
mod summary_tests;

#[cfg(test)]
#[path = "changes_projection_tests/withdrawn.rs"]
mod withdrawn_tests;
