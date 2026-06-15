//! The `changes` Postgres table is the projection of change-lifecycle events.
//! Reads are SQL queries returning [`Change`]; writes (`write_*`) are called
//! by `event_bus::update_thread_projection` inside the same transaction as the
//! `thread_summaries` update — write-through keeps the table current with the
//! events stream. [`ChangesProjection::rebuild_missing_from_events`] is an
//! idempotent safety net invoked at startup; it recovers rows for any
//! change_id present in `events` but absent from `changes`.

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::core::changes::{Change, RestartGroup};

/// Column projection used by every `query_as::<_, Change>` call. The table has
/// no `thread_title` column — it's enriched per-response from `thread_summaries`
/// (see `crate::core::changes::enrich_thread_titles`).
const SELECT_CHANGE: &str = "SELECT id, request_id, thread_id, branch_name, repo_root, description, \
     file_count, files, requires_restart, status, created_at, resolved_at, \
     merge_worktree_path, merge_temp_branch, hardened, pre_merge_sha, \
     post_merge_sha, NULL::text AS thread_title, commits, incomplete FROM changes";

#[derive(Clone)]
pub struct ChangesProjection {
    pool: PgPool,
}

/// Outcome of the in-tx single-fire check for a `ChangeApplied` emit
/// ([`ChangesProjection::change_apply_dedup`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChangeApplyDedup {
    /// First (or only) apply for this change_id — persist the event.
    Proceed,
    /// A `ChangeApplied` was already recorded for this change_id (the row is
    /// already `applied`); a concurrent or later duplicate apply lost the race.
    /// The caller must skip persisting a second event.
    Suppress,
}

/// Parse change_id silently. Real emit paths always pass a valid UUID; tests
/// frequently pass throwaway strings ("c1") whose target row doesn't exist —
/// the matching SQL helper then no-ops. Diagnostics for "id parsed but no
/// such row" live on the helpers that care (e.g. `write_status` /
/// `write_applied`) via a `rows_affected() == 0` check.
fn parse_change_id(change_id: &str) -> Option<Uuid> {
    Uuid::parse_str(change_id).ok()
}

impl ChangesProjection {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // --- Write API (called from event_bus inside the event commit tx) ---

    /// Insert (or update on re-emit) the row for an aggregate `ChangeProposed`.
    /// `request_id` is `Uuid::nil()` — the event payload doesn't carry it; auto-apply
    /// correlation is dead code today. Revive: extend the event variant first.
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
    ) -> sqlx::Result<()> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(());
        };
        // description bound as Option<&str>: COALESCE on insert defaults to ''
        // (column is NOT NULL); COALESCE on update preserves the existing
        // description when the re-emit carries None — matches the in-memory
        // contract `if let Some(d) = description { existing.description = d }`.
        // `incomplete` propagates verbatim on re-emit so a follow-up
        // successful turn against the same branch clears a prior failure tag.
        sqlx::query(
            "INSERT INTO changes (id, request_id, thread_id, branch_name, repo_root, \
             description, file_count, files, requires_restart, status, created_at, hardened, incomplete) \
             VALUES ($1, $2, $3, $4, $5, COALESCE($6, ''), $7, $8, $9, 'pending', NOW(), $10, $11) \
             ON CONFLICT (id) DO UPDATE SET \
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
        .execute(&mut **tx)
        .await?;
        Ok(())
    }

    /// Per-commit `ChangeProposed` (empty change_id, commit_sha set): bump
    /// description, OR the restart flag, and invalidate the harden marker on
    /// the still-pending row matched by branch. Replay-only — no live
    /// emitter — and UPDATE-only so replay can never create a row.
    pub(crate) async fn write_proposed_per_commit(
        tx: &mut Transaction<'_, Postgres>,
        branch_name: &str,
        description: Option<&str>,
        requires_restart: bool,
    ) -> sqlx::Result<()> {
        sqlx::query(
            "UPDATE changes SET \
             description = COALESCE($2, description), \
             requires_restart = requires_restart OR $3, \
             hardened = FALSE \
             WHERE branch_name = $1 AND status = 'pending'",
        )
        .bind(branch_name)
        .bind(description)
        .bind(requires_restart)
        .execute(&mut **tx)
        .await?;
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
        let res = sqlx::query(
            "UPDATE changes SET status = 'applied', resolved_at = NOW(), \
             requires_restart = $2, \
             commits = CASE WHEN cardinality($3::text[]) > 0 THEN $3 ELSE commits END, \
             pre_merge_sha = COALESCE($4, pre_merge_sha), \
             post_merge_sha = COALESCE($5, post_merge_sha) \
             WHERE id = $1",
        )
        .bind(id)
        .bind(requires_restart)
        .bind(commits)
        .bind(pre_merge_sha)
        .bind(post_merge_sha)
        .execute(&mut **tx)
        .await?;
        if res.rows_affected() == 0 {
            crate::log!("[ChangesProjection] write_applied: no row for {} — missing aggregate ChangeProposed?", id);
        }
        Ok(())
    }

    /// Single-fire guard for `ChangeApplied`. Locks the change row `FOR UPDATE`
    /// and decides whether the surrounding emit should persist the event.
    ///
    /// The change row's `status` is the claim token: [`write_applied`] (run in
    /// the same emit transaction, Phase Project) flips it `pending`→`applied`,
    /// and the `FOR UPDATE` lock taken here is held until that transaction
    /// commits. A concurrent or later `ChangeApplied` for the same change_id
    /// therefore blocks on the lock, then reads `applied` and gets
    /// [`ChangeApplyDedup::Suppress`] — so one logical apply can never persist
    /// two `ChangeApplied` events (the two-"Change applied"-entries bug).
    ///
    /// A missing or unparseable change_id yields [`ChangeApplyDedup::Proceed`]:
    /// there's nothing to dedup against (throwaway test ids, the external-repo
    /// archive carve-out, or the Tier-3 slow path whose `ChangeProposed` row was
    /// never created), so existing single-emit behavior is preserved.
    ///
    /// [`write_applied`]: Self::write_applied
    pub(crate) async fn change_apply_dedup(
        tx: &mut Transaction<'_, Postgres>,
        change_id: &str,
    ) -> sqlx::Result<ChangeApplyDedup> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(ChangeApplyDedup::Proceed);
        };
        let status: Option<String> =
            sqlx::query_scalar("SELECT status FROM changes WHERE id = $1 FOR UPDATE")
                .bind(id)
                .fetch_optional(&mut **tx)
                .await?;
        Ok(if status.as_deref() == Some("applied") {
            ChangeApplyDedup::Suppress
        } else {
            ChangeApplyDedup::Proceed
        })
    }

    pub(crate) async fn write_status(
        tx: &mut Transaction<'_, Postgres>,
        change_id: &str,
        status: &str,
    ) -> sqlx::Result<()> {
        let Some(id) = parse_change_id(change_id) else {
            return Ok(());
        };
        let res = sqlx::query("UPDATE changes SET status = $2, resolved_at = NOW() WHERE id = $1")
            .bind(id)
            .bind(status)
            .execute(&mut **tx)
            .await?;
        if res.rows_affected() == 0 {
            crate::log!("[ChangesProjection] write_status({}) for {}: no row — missing aggregate ChangeProposed?", status, id);
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

    // --- Read API ---
    //
    // Every read returns `Result<_, sqlx::Error>` so a DB outage is
    // distinguishable from "no rows". Degradation lives at the call site,
    // not in the projection.

    /// All pending changes (status = "pending"), ordered by created_at ASC.
    pub async fn list_pending(&self) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status = 'pending' ORDER BY created_at ASC"
        ))
        .fetch_all(&self.pool)
        .await
    }

    /// Get a single change by id.
    pub async fn get_by_id(&self, id: Uuid) -> sqlx::Result<Option<Change>> {
        sqlx::query_as(&format!("{SELECT_CHANGE} WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
    }

    /// Get the pending change for a given branch, if one exists.
    pub async fn get_pending_by_branch(&self, branch_name: &str) -> sqlx::Result<Option<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE branch_name = $1 AND status = 'pending' LIMIT 1"
        ))
        .bind(branch_name)
        .fetch_optional(&self.pool)
        .await
    }

    /// All pending changes for a specific thread.
    pub async fn pending_for_thread(&self, thread_id: Uuid) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status = 'pending' AND thread_id = $1 ORDER BY created_at ASC"
        ))
        .bind(thread_id)
        .fetch_all(&self.pool)
        .await
    }

    /// Whether any pending change exists for the given branch.
    pub async fn has_pending_for_branch(&self, branch_name: &str) -> sqlx::Result<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM changes WHERE status = 'pending' AND branch_name = $1)",
        )
        .bind(branch_name)
        .fetch_one(&self.pool)
        .await
    }

    /// Whether any OTHER pending change exists for the given branch (excluding `exclude_id`).
    pub async fn other_pending_for_branch(
        &self,
        branch_name: &str,
        exclude_id: Uuid,
    ) -> sqlx::Result<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM changes \
             WHERE status = 'pending' AND branch_name = $1 AND id <> $2)",
        )
        .bind(branch_name)
        .bind(exclude_id)
        .fetch_one(&self.pool)
        .await
    }

    /// Distinct branch names with at least one applied or discarded change.
    pub async fn list_completed_branches(&self) -> sqlx::Result<Vec<String>> {
        sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT branch_name FROM changes WHERE status IN ('applied', 'discarded')",
        )
        .fetch_all(&self.pool)
        .await
    }

    /// Recently applied or reverted changes, newest `resolved_at` first.
    /// `before`: only items with `resolved_at < before`. `limit`: max count.
    pub async fn list_recently_applied(
        &self,
        limit: i64,
        before: Option<DateTime<Utc>>,
    ) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status IN ('applied', 'reverted') \
             AND ($2::timestamptz IS NULL OR resolved_at < $2) \
             ORDER BY resolved_at DESC NULLS LAST LIMIT $1"
        ))
        .bind(limit.max(0))
        .bind(before)
        .fetch_all(&self.pool)
        .await
    }

    /// Pending + recently applied/reverted changes for a single repo.
    /// Returns `(pending, applied, has_more)` — fetches `applied_limit + 1`
    /// internally so `has_more` reflects whether more applied items exist.
    /// A failure on either query fails the whole call: partial results would
    /// silently underreport `has_more` or hide pending changes.
    pub async fn list_for_repo(
        &self,
        repo_root: &str,
        applied_limit: i64,
        before: Option<DateTime<Utc>>,
    ) -> sqlx::Result<(Vec<Change>, Vec<Change>, bool)> {
        let fetch_limit = applied_limit.max(0) + 1;
        let pending_sql = format!(
            "{SELECT_CHANGE} WHERE status = 'pending' AND repo_root = $1 \
             ORDER BY created_at ASC"
        );
        let applied_sql = format!(
            "{SELECT_CHANGE} WHERE status IN ('applied', 'reverted') AND repo_root = $1 \
             AND ($3::timestamptz IS NULL OR resolved_at < $3) \
             ORDER BY resolved_at DESC NULLS LAST LIMIT $2"
        );
        let (pending_r, applied_r) = tokio::join!(
            sqlx::query_as(&pending_sql).bind(repo_root).fetch_all(&self.pool),
            sqlx::query_as(&applied_sql)
                .bind(repo_root)
                .bind(fetch_limit)
                .bind(before)
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

    /// Whether any restart-required change has been applied since `since`.
    /// Used to derive the in-app "restart required" toast from durable state.
    pub async fn requires_restart_since(&self, since: DateTime<Utc>) -> sqlx::Result<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM changes \
             WHERE status = 'applied' AND requires_restart AND resolved_at > $1)",
        )
        .bind(since)
        .fetch_one(&self.pool)
        .await
    }

    /// Whether any applied change since `since` touches frontend files
    /// (`.ts`, `.tsx`, `.css`, `.html`, `.js`, `.jsx`). Used to nudge clients
    /// to reload after a hot-fix.
    pub async fn client_update_since(&self, since: DateTime<Utc>) -> sqlx::Result<bool> {
        sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM changes c, unnest(c.files) AS f \
             WHERE c.status = 'applied' AND c.resolved_at > $1 \
             AND (f LIKE '%.ts' OR f LIKE '%.tsx' OR f LIKE '%.css' \
                  OR f LIKE '%.html' OR f LIKE '%.js' OR f LIKE '%.jsx'))",
        )
        .bind(since)
        .fetch_one(&self.pool)
        .await
    }

    /// Restart groups (one per originating thread) for restart-required
    /// applied changes since `since`. Commits from multiple changes on the
    /// same thread are merged in apply order. `thread_title` is left `None`;
    /// callers enrich via `thread_summaries` if needed.
    pub async fn restart_groups_since(
        &self,
        since: DateTime<Utc>,
    ) -> sqlx::Result<Vec<RestartGroup>> {
        let rows: Vec<(Option<Uuid>, Vec<String>)> = sqlx::query_as(
            "SELECT thread_id, commits FROM changes \
             WHERE status = 'applied' AND requires_restart AND resolved_at > $1 \
             ORDER BY resolved_at ASC",
        )
        .bind(since)
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

    /// Pending changes that have an active merge worktree (used for
    /// startup cleanup of stale merge dirs).
    pub async fn with_merge_worktree(&self) -> sqlx::Result<Vec<Change>> {
        sqlx::query_as(&format!(
            "{SELECT_CHANGE} WHERE status = 'pending' AND merge_worktree_path IS NOT NULL"
        ))
        .fetch_all(&self.pool)
        .await
    }

    /// Recover change rows referenced by `events` but missing from `changes`
    /// by replaying their lifecycle into one synthesized row per gap.
    /// Idempotent (`ON CONFLICT (id) DO NOTHING`); a healthy projection
    /// returns 0. Backed by `events_change_id_idx` for the seed scan.
    pub async fn rebuild_missing_from_events(&self) -> sqlx::Result<usize> {
        let missing_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT DISTINCT (payload->>'change_id')::uuid AS change_id
             FROM events
             WHERE event_type IN (
                 'ChangeProposed','ChangeApplied','ChangeDiscarded',
                 'ChangeReverted','ChangeHardened',
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
        crate::log!(
            "[ChangesProjection] rebuild_missing_from_events: recovered {}/{} change rows",
            recovered,
            missing_ids.len()
        );
        Ok(recovered)
    }
}

/// Returns `true` on insert, `false` if the change has no aggregate
/// `ChangeProposed` to seed from. `ON CONFLICT (id) DO NOTHING` so a
/// concurrent normal write or a re-run is safe.
async fn rebuild_one_from_events(pool: &PgPool, change_id: Uuid) -> sqlx::Result<bool> {
    let rows: Vec<(String, serde_json::Value, Option<Uuid>, DateTime<Utc>)> = sqlx::query_as(
        "SELECT event_type, payload, thread_id, created
         FROM events
         WHERE payload->>'change_id' = $1
           AND event_type IN (
               'ChangeProposed','ChangeApplied','ChangeDiscarded',
               'ChangeReverted','ChangeHardened',
               'MergeResolutionStarted','MergeResolutionCleared'
           )
         ORDER BY created",
    )
    .bind(change_id.to_string())
    .fetch_all(pool)
    .await?;

    // Per-commit ChangeProposed emits carry an empty change_id (filtered out
    // upstream by the uuid regex), so any ChangeProposed reaching us here is
    // an aggregate. The `commit_sha` guard is defense in depth.
    let is_aggregate_proposed = |r: &(String, serde_json::Value, Option<Uuid>, DateTime<Utc>)| {
        r.0 == "ChangeProposed"
            && r.1
                .get("commit_sha")
                .map(|v| v.is_null())
                .unwrap_or(true)
    };

    let Some(first) = rows.iter().find(|r| is_aggregate_proposed(r)) else {
        crate::log!(
            "[ChangesProjection] rebuild_one: change {} has no aggregate ChangeProposed — skipping",
            change_id
        );
        return Ok(false);
    };
    // Latest aggregate ChangeProposed wins for description/files/restart/hardened.
    // Invariant: the forward `find` above returned Some, so the same predicate
    // applied in reverse over the same `rows` slice must also match — the last
    // matching row (= the "latest" we want here) is therefore guaranteed.
    let latest = rows
        .iter()
        .rev()
        .find(|r| is_aggregate_proposed(r))
        .expect("forward find above guarantees at least one aggregate ChangeProposed row");

    let str_field = |p: &serde_json::Value, k: &str| {
        p.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string()
    };
    let bool_field = |p: &serde_json::Value, k: &str| {
        p.get(k).and_then(|v| v.as_bool()).unwrap_or(false)
    };
    let str_array = |p: &serde_json::Value, k: &str| -> Vec<String> {
        p.get(k)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
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

    let terminal = rows
        .iter()
        .rev()
        .find(|r| matches!(r.0.as_str(), "ChangeApplied" | "ChangeDiscarded" | "ChangeReverted"));

    // Active merge worktree state: the latest unpaired MergeResolutionStarted.
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
        Some(t) => {
            let status = match t.0.as_str() {
                "ChangeApplied" => "applied",
                "ChangeDiscarded" => "discarded",
                "ChangeReverted" => "reverted",
                _ => unreachable!(),
            };
            if t.0 == "ChangeApplied" {
                requires_restart = bool_field(&t.1, "requires_restart") || requires_restart;
            }
            let commits = str_array(&t.1, "commits");
            let pre = t.1.get("pre_merge_sha").and_then(|v| v.as_str()).map(String::from);
            let post = t.1.get("post_merge_sha").and_then(|v| v.as_str()).map(String::from);
            (status, Some(t.3), commits, pre, post)
        }
        None => ("pending", None, Vec::new(), None, None),
    };

    let res = sqlx::query(
        "INSERT INTO changes (id, request_id, thread_id, branch_name, repo_root,
         description, file_count, files, requires_restart, status, created_at,
         resolved_at, hardened, commits, pre_merge_sha, post_merge_sha,
         merge_worktree_path, merge_temp_branch, incomplete)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19)
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
    .execute(pool)
    .await?;

    // ON CONFLICT skip means a concurrent write beat us — don't claim recovery.
    Ok(res.rows_affected() > 0)
}

#[cfg(test)]
#[path = "changes_projection_tests/helpers.rs"]
mod cp_helpers;

#[cfg(test)]
#[path = "changes_projection_tests/projection.rs"]
mod projection_tests;

#[cfg(test)]
#[path = "changes_projection_tests/queries.rs"]
mod queries_tests;

#[cfg(test)]
#[path = "changes_projection_tests/rebuild.rs"]
mod rebuild_tests;
