use std::collections::HashMap;

use uuid::Uuid;

use crate::engine::thread_events::CC_ORIGINATING_EVENT_TYPES;
use crate::engine::thread_events::{AbortCause, EventChannel, ThreadEvent};

const MAX_LINES: usize = 50;

const MAX_LINES_PER_EVENT: usize = 20;

const COVERED_EVENT_TYPES: &[&str] = &[
    "ChangeApplied",
    "ChangeDiscarded",
    "ChangeReverted",
    "ChangeApplyFailed",
    "WorktreeCleaned",
    "BackgroundBashCompleted",
    "ChildThreadStopped",
    "ChildThreadDetached",
    "ChildReportFiled",
];

const EXTRA_BOUNDARY_EVENT_TYPES: &[&str] = &["CodingAgentPromptSent"];

const USER_ONLY_MESSAGE_EVENT_TYPE: &str = "UserOnlyMessagePosted";

const MAX_USER_ONLY_MESSAGES: usize = 50;

const MAX_USER_ONLY_MESSAGE_CHARS: usize = 2000;

const MAX_DESCRIPTION_CHARS: usize = 80;

/// INVARIANT: How long after its own stand-down a killed task's completion still counts
/// as the thread's own stop. The reap takes the 3 s stop grace plus the drain,
/// so a minute is generous even on a saturated host.
const SELF_STOP_REAP_WINDOW_SECS: i64 = 60;

const MAX_ERROR_CHARS: usize = 200;

pub(crate) struct TurnGapNote {
    pub note: String,
    pub explains_worktree_reset: bool,
    pub user_event_ids: Vec<Uuid>,
}

enum GapEvent {
    Applied {
        change_id: String,
        commits: Vec<String>,
        short_sha: Option<String>,
    },
    Discarded {
        change_id: String,
    },
    Reverted {
        change_id: String,
    },
    ApplyFailed {
        change_id: String,
        error: String,
    },
    WorktreeCleaned {
        tier: u8,
        freed_bytes: u64,
        branch_deleted: bool,
    },
    BackgroundTaskEnded {
        task_id: String,
        command: String,
        abandoned: bool,
        timed_out: bool,
        killed: bool,
        status: String,
        started_at: String,
    },
    ChildStopped {
        child_thread_id: String,
        title: String,
    },
    ChildMovedOut {
        child_thread_id: String,
        title: String,
    },
    ChildReportFiled {
        block: String,
    },
    RestartCutOffLastTurn,
}

impl GapEvent {
    fn change_id(&self) -> Option<&str> {
        match self {
            Self::Applied { change_id, .. }
            | Self::Discarded { change_id }
            | Self::Reverted { change_id }
            | Self::ApplyFailed { change_id, .. } => Some(change_id),
            Self::WorktreeCleaned { .. }
            | Self::BackgroundTaskEnded { .. }
            | Self::ChildStopped { .. }
            | Self::ChildMovedOut { .. }
            | Self::ChildReportFiled { .. }
            | Self::RestartCutOffLastTurn => None,
        }
    }

    fn moved_the_worktree(&self, branch: &str, session_branch: Option<&str>) -> bool {
        match self {
            Self::Applied { .. } | Self::Discarded { .. } => {
                branch.is_empty() || session_branch.is_none_or(|current| current == branch)
            }
            Self::WorktreeCleaned { tier, .. } => *tier != 1,
            Self::Reverted { .. }
            | Self::ApplyFailed { .. }
            | Self::BackgroundTaskEnded { .. }
            | Self::ChildStopped { .. }
            | Self::ChildMovedOut { .. }
            | Self::ChildReportFiled { .. }
            | Self::RestartCutOffLastTurn => false,
        }
    }
}

fn branch_for<'a>(event: &GapEvent, facts: &'a HashMap<Uuid, ChangeFacts>) -> &'a str {
    facts_for(event, facts)
        .map(|f| f.branch_name.as_str())
        .unwrap_or("")
}

fn facts_for<'a>(
    event: &GapEvent,
    facts: &'a HashMap<Uuid, ChangeFacts>,
) -> Option<&'a ChangeFacts> {
    event
        .change_id()
        .and_then(|id| Uuid::parse_str(id).ok())
        .and_then(|id| facts.get(&id))
}

struct ChangeFacts {
    branch_name: String,
    description: String,
}

fn boundary_event_types() -> Vec<String> {
    CC_ORIGINATING_EVENT_TYPES
        .iter()
        .map(|s| (*s).to_string())
        .chain(EXTRA_BOUNDARY_EVENT_TYPES.iter().map(|s| (*s).to_string()))
        .collect()
}

pub(crate) async fn compute_turn_gap_note(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    current_origin_id: Uuid,
    session_branch: Option<&str>,
) -> Option<TurnGapNote> {
    compute_turn_gap_note_until(
        pool,
        thread_id,
        current_origin_id,
        current_origin_id,
        session_branch,
    )
    .await
}

pub(crate) async fn compute_turn_gap_note_until(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    anchor_id: Uuid,
    until_id: Uuid,
    session_branch: Option<&str>,
) -> Option<TurnGapNote> {
    let (anchor_sequence, until_sequence): (Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT (SELECT sequence FROM events WHERE id = $1), \
                (SELECT sequence FROM events WHERE id = $2)",
    )
    .bind(anchor_id)
    .bind(until_id)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| {
        log_legacy!(
            "[AgentSession] Failed to resolve turn-gap bounds for {}: {}",
            thread_id,
            e
        );
        (None, None)
    });

    let rows: Vec<(Uuid, String, serde_json::Value)> =
        sqlx::query_as::<_, (Uuid, String, serde_json::Value)>(&format!(
            "SELECT e.id, e.event_type, \
                CASE WHEN e.event_type = $9 THEN jsonb_set(e.payload, '{{text}}', \
                       to_jsonb(left(COALESCE(e.payload->>'text', ''), $10))) \
                     ELSE e.payload - 'stdout' - 'stderr' - 'text' END \
             FROM events e \
         WHERE e.thread_id = $1 AND e.event_type = ANY($2) \
           AND ($7::bigint IS NULL OR e.sequence < $7) \
           AND e.sequence > COALESCE(( \
             SELECT b.sequence FROM events b \
             WHERE b.thread_id = $1 \
               AND b.id <> ALL($4) \
               AND ($3::bigint IS NULL OR b.sequence < $3) \
               AND b.event_type = ANY($5) \
               AND (b.payload->'origin'->'reason'->>'kind') IS DISTINCT FROM $8 \
               AND (COALESCE(b.created < ( \
                     SELECT MIN(f.created) FROM events f \
                     WHERE f.event_type = 'CodingAgentInputRead' \
                   ), TRUE) \
                 OR EXISTS ( \
                   SELECT 1 FROM events r \
                   WHERE r.thread_id = $1 AND r.event_type = 'CodingAgentInputRead' \
                     AND r.payload->>'assumed' IS DISTINCT FROM 'true' \
                     AND r.payload->>'verbatim' IS DISTINCT FROM 'true' \
                     AND r.payload->>'input_event_id' \
                       IN (b.id::text, b.payload->>'input_event_id') \
                 )) \
             ORDER BY b.sequence DESC LIMIT 1 \
           ), 0) \
           AND NOT (e.event_type = 'BackgroundBashCompleted' AND EXISTS ( \
             SELECT 1 FROM events d \
             WHERE d.thread_id = $1 AND d.event_type = 'EventWaitDelivered' \
               AND d.payload->>'event_id' = e.id::text \
           )) \
           AND NOT (e.event_type = 'BackgroundBashCompleted' AND e.sequence < COALESCE(( \
             SELECT max(i.sequence) FROM events i \
             WHERE i.thread_id = $1 AND i.event_type = 'CodingAgentIdled' \
               AND {not_engine_turn_idle} \
               AND ($3::bigint IS NULL OR i.sequence < $3) \
           ), 0)) \
           AND NOT (e.event_type = 'BackgroundBashCompleted' \
             AND e.payload->>'killed' = 'true' AND EXISTS ( \
               SELECT 1 FROM events c, jsonb_array_elements(c.payload->'on') watched \
               WHERE c.thread_id = $1 AND c.event_type = 'EventWaitCanceled' \
                 AND c.payload->>'cause' = 'agent_stand_down' \
                 AND c.sequence < e.sequence \
                 AND c.created >= e.created - make_interval(secs => $6) \
                 AND watched->'condition'->>'task_id' = e.payload->>'task_id' \
           )) \
         ORDER BY e.sequence ASC",
            not_engine_turn_idle = crate::engine::thread_events::not_an_engine_turn_idle_sql("i.")
        ))
        .bind(thread_id)
        .bind(
            COVERED_EVENT_TYPES
                .iter()
                .chain(ThreadEvent::TERMINATOR_EVENT_TYPES)
                .chain(&[USER_ONLY_MESSAGE_EVENT_TYPE])
                .copied()
                .collect::<Vec<&str>>(),
        )
        .bind(anchor_sequence)
        .bind([anchor_id, until_id])
        .bind(boundary_event_types())
        .bind(SELF_STOP_REAP_WINDOW_SECS)
        .bind(until_sequence)
        .bind(crate::engine::thread_events::CACHE_KEEP_ALIVE_REASON_KIND)
        .bind(USER_ONLY_MESSAGE_EVENT_TYPE)
        .bind((MAX_USER_ONLY_MESSAGE_CHARS + 1) as i32)
        .fetch_all(pool)
        .await
        .map_err(|e| {
            log_legacy!(
                "[AgentSession] Failed to look up turn-gap events for {}: {}",
                thread_id,
                e
            );
            e
        })
        .ok()?;

    let (user_only, rows): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|(_, event_type, _)| event_type == USER_ONLY_MESSAGE_EVENT_TYPE);
    let user_event_ids: Vec<Uuid> = user_only.iter().map(|(id, _, _)| *id).collect();
    let user_talk = user_talk_block(
        &user_only
            .into_iter()
            .map(|(_, _, payload)| payload)
            .collect::<Vec<_>>(),
        &crate::gateway_users::current(),
    );
    let (terminals, covered): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(|(_, event_type, _)| {
            ThreadEvent::TERMINATOR_EVENT_TYPES.contains(&event_type.as_str())
        });
    let terminals: Vec<(String, serde_json::Value)> = terminals
        .into_iter()
        .map(|(_, event_type, payload)| (event_type, payload))
        .collect();

    let mut events: Vec<GapEvent> = covered.into_iter().filter_map(parse_gap_event).collect();
    if last_turn_cut_off_by_restart(&terminals) {
        events.insert(0, GapEvent::RestartCutOffLastTurn);
    }

    if events.is_empty() {
        return user_talk.map(|note| TurnGapNote {
            note,
            explains_worktree_reset: false,
            user_event_ids,
        });
    }

    let facts = load_change_facts(pool, &events).await;
    let explains_worktree_reset = events
        .iter()
        .any(|e| e.moved_the_worktree(branch_for(e, &facts), session_branch));
    let gap_note = build_note(&events, &facts, session_branch);

    Some(TurnGapNote {
        note: match user_talk {
            Some(talk) => format!("{gap_note}\n\n{talk}"),
            None => gap_note,
        },
        explains_worktree_reset,
        user_event_ids,
    })
}

fn user_talk_block(
    payloads: &[serde_json::Value],
    users: &crate::gateway_users::UserSnapshot,
) -> Option<String> {
    if payloads.is_empty() {
        return None;
    }
    let skipped = payloads.len().saturating_sub(MAX_USER_ONLY_MESSAGES);
    let mut block = String::from(
        "[User-only messages: since your previous prompt, people in this thread talked to \
         each other without waking you. Read them for context. Answer only what the message \
         below asks.",
    );
    if skipped > 0 {
        block.push_str(&format!("\n… {skipped} earlier message(s) not shown"));
    }
    for payload in &payloads[skipped..] {
        let device = crate::core::store::typed_on_device(USER_ONLY_MESSAGE_EVENT_TYPE, payload);
        let speaker = users.speaker_of_device(device.as_deref());
        let mut text = str_field(payload, "text");
        if let Some((cut, _)) = text.char_indices().nth(MAX_USER_ONLY_MESSAGE_CHARS) {
            text.truncate(cut);
            text.push_str("… [cut]");
        }
        block.push_str(&format!("\n{speaker}: {text}"));
    }
    block.push(']');
    Some(block)
}

fn last_turn_cut_off_by_restart(terminals: &[(String, serde_json::Value)]) -> bool {
    terminals
        .iter()
        .rev()
        .find(|(_, payload)| {
            payload.get("channel").and_then(|v| v.as_str())
                == Some(EventChannel::ClaudeCode.as_str())
        })
        .is_some_and(|(event_type, payload)| {
            event_type == "ResponseAborted"
                && matches!(
                    payload
                        .get("cause")
                        .map(|c| serde_json::from_value::<AbortCause>(c.clone())),
                    Some(Ok(
                        AbortCause::EngineShutdown | AbortCause::RecoveryAfterRestart
                    ))
                )
        })
}

fn parse_gap_event(
    (event_id, event_type, payload): (Uuid, String, serde_json::Value),
) -> Option<GapEvent> {
    let change_id = str_field(&payload, "change_id");
    match event_type.as_str() {
        "ChangeApplied" => Some(GapEvent::Applied {
            change_id,
            commits: payload
                .get("commits")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            short_sha: payload
                .get("post_merge_sha")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(short),
        }),
        "ChangeDiscarded" => Some(GapEvent::Discarded { change_id }),
        "ChangeReverted" => Some(GapEvent::Reverted { change_id }),
        "ChangeApplyFailed" => Some(GapEvent::ApplyFailed {
            change_id,
            error: truncate(&str_field(&payload, "error"), MAX_ERROR_CHARS),
        }),
        "WorktreeCleaned" => Some(GapEvent::WorktreeCleaned {
            tier: payload
                .get("tier")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .min(u64::from(u8::MAX)) as u8,
            freed_bytes: payload
                .get("freed_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            branch_deleted: payload
                .get("branch_deleted")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }),
        "BackgroundBashCompleted" => Some(GapEvent::BackgroundTaskEnded {
            task_id: str_field(&payload, "task_id"),
            command: truncate(&str_field(&payload, "command"), MAX_DESCRIPTION_CHARS),
            started_at: str_field(&payload, "started_at"),
            abandoned: bool_field(&payload, "abandoned"),
            timed_out: bool_field(&payload, "timed_out"),
            killed: bool_field(&payload, "killed"),
            status: match (
                int_field(&payload, "exit_code"),
                int_field(&payload, "signal"),
            ) {
                (None, None) if !payload.get("exit_code").is_some_and(|v| v.is_null()) => {
                    String::new()
                }
                (exit_code, signal) => {
                    crate::core::shell::TaskOutcome::from_persisted(exit_code, signal).describe()
                }
            },
        }),
        "ChildThreadStopped" => Some(GapEvent::ChildStopped {
            child_thread_id: str_field(&payload, "child_thread_id"),
            title: truncate(
                &str_field(&payload, "child_thread_title"),
                MAX_DESCRIPTION_CHARS,
            ),
        }),
        "ChildThreadDetached" => Some(GapEvent::ChildMovedOut {
            child_thread_id: str_field(&payload, "child_thread_id"),
            title: truncate(
                &str_field(&payload, "child_thread_title"),
                MAX_DESCRIPTION_CHARS,
            ),
        }),
        "ChildReportFiled" => Some(GapEvent::ChildReportFiled {
            block: crate::core::store::messages::format_child_report_filed_block(
                &crate::core::events::EventRow {
                    id: event_id,
                    ..crate::core::events::EventRow::new(&event_type, payload)
                },
                &crate::core::store::ChangeStatuses::new(),
            ),
        }),
        _ => None,
    }
}

fn bool_field(payload: &serde_json::Value, key: &str) -> bool {
    payload.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

fn int_field(payload: &serde_json::Value, key: &str) -> Option<i32> {
    payload.get(key).and_then(|v| v.as_i64()).map(|n| n as i32)
}

fn str_field(payload: &serde_json::Value, key: &str) -> String {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

async fn load_change_facts(pool: &sqlx::PgPool, events: &[GapEvent]) -> HashMap<Uuid, ChangeFacts> {
    let ids: Vec<Uuid> = events
        .iter()
        .filter_map(GapEvent::change_id)
        .filter_map(|id| Uuid::parse_str(id).ok())
        .collect();
    if ids.is_empty() {
        return HashMap::new();
    }

    sqlx::query_as::<_, (Uuid, String, String)>(
        "SELECT id, branch_name, description FROM changes WHERE id = ANY($1)",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await
    .map_err(|e| {
        log_legacy!(
            "[AgentSession] Failed to resolve change facts for the turn-gap note: {}",
            e
        );
        e
    })
    .map(|rows| {
        rows.into_iter()
            .map(|(id, branch_name, description)| {
                (
                    id,
                    ChangeFacts {
                        branch_name,
                        description,
                    },
                )
            })
            .collect()
    })
    .unwrap_or_default()
}

fn short(s: &str) -> String {
    s[..s.floor_char_boundary(8)].to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}…")
}

fn build_note(
    events: &[GapEvent],
    facts: &HashMap<Uuid, ChangeFacts>,
    session_branch: Option<&str>,
) -> String {
    let mut note = String::from(
        "[Note from engine: since your previous prompt, the user or the engine acted on your \
         work. Reconcile what you are about to say with this:",
    );

    let mut lines = 0usize;
    let mut truncated = 0usize;

    let (untraced, rest): (Vec<&GapEvent>, Vec<&GapEvent>) = events.iter().partition(|e| {
        matches!(
            e,
            GapEvent::RestartCutOffLastTurn | GapEvent::BackgroundTaskEnded { .. }
        )
    });

    for event in untraced.into_iter().chain(rest) {
        let branch = branch_for(event, facts);
        let label = change_label(event.change_id().unwrap_or(""), facts_for(event, facts));

        let mut spent_here = 0usize;
        for line in render(event, &label, branch, session_branch) {
            if lines < MAX_LINES && spent_here < MAX_LINES_PER_EVENT {
                note.push('\n');
                note.push_str(&line);
                lines += 1;
                spent_here += 1;
            } else {
                truncated += 1;
            }
        }
    }

    if truncated > 0 {
        note.push_str(&format!("\n… and {} more line(s) not shown", truncated));
    }

    for line in closing_guidance(events) {
        note.push('\n');
        note.push_str(line);
    }
    note.push(']');
    note
}

fn render(
    event: &GapEvent,
    label: &str,
    branch: &str,
    session_branch: Option<&str>,
) -> Vec<String> {
    match event {
        GapEvent::Applied {
            change_id,
            commits,
            short_sha,
        } => {
            if commits.is_empty() {
                return vec![format!(
                    "- APPLIED: change {}: no commits / already merged.",
                    short_change_id(change_id)
                )];
            }
            let sha_suffix = match short_sha {
                Some(s) => format!(" (now in main at {})", s),
                None => " (now in main)".to_string(),
            };
            commits
                .iter()
                .map(|subject| format!("- APPLIED: {}{}", subject, sha_suffix))
                .collect()
        }
        GapEvent::Discarded { .. } => vec![discarded_line(label, branch, session_branch)],
        GapEvent::Reverted { .. } => vec![format!(
            "- REVERTED: {}{} had been applied and has now been undone in main by revert \
             commits. main no longer contains that work; your branch and worktree were not \
             touched by the revert.",
            label,
            on_branch(branch)
        )],
        GapEvent::ApplyFailed { error, .. } => vec![format!(
            "- APPLY FAILED: {}{} is STILL PENDING; the Apply attempt did not land. The engine \
             showed the user this message: \"{}\"",
            label,
            on_branch(branch),
            error
        )],
        GapEvent::WorktreeCleaned {
            tier,
            freed_bytes,
            branch_deleted,
        } => vec![worktree_cleaned_line(*tier, *freed_bytes, *branch_deleted)],
        GapEvent::BackgroundTaskEnded {
            task_id,
            command,
            abandoned,
            timed_out,
            killed,
            status,
            started_at,
        } => vec![background_task_line(BackgroundTaskLine {
            command,
            task_id,
            abandoned: *abandoned,
            timed_out: *timed_out,
            killed: *killed,
            status,
            started_at,
        })],
        GapEvent::ChildStopped {
            child_thread_id,
            title,
        } => vec![child_stopped_line(child_thread_id, title)],
        GapEvent::ChildMovedOut {
            child_thread_id,
            title,
        } => vec![child_moved_out_line(child_thread_id, title)],
        GapEvent::ChildReportFiled { block } => vec![format!("- {block}")],
        GapEvent::RestartCutOffLastTurn => vec![RESTART_CUT_OFF_LINE.to_string()],
    }
}

/// INVARIANT: The bullet for a turn an engine restart cut off. It quotes the text Claude
/// Code writes at teardown, so the agent recognises the lines it must discount.
const RESTART_CUT_OFF_LINE: &str = "- ENGINE RESTART: the Lucidos engine restarted while your \
    last turn was running, and it stopped your session mid-turn. Your transcript may show a tool \
    call rejected with a line saying the user doesn't want to proceed and that you should stop \
    and wait. It may also show \"[Request interrupted by user for tool use]\" or \"[Request \
    interrupted by user]\". The engine's shutdown wrote those lines, not the user: nobody refused \
    or interrupted anything. A tool call that was running did not complete. It is safe to run it \
    again: first check whether it had side effects that may have partly happened, then carry on \
    with your plan.";

fn child_stopped_line(child_thread_id: &str, title: &str) -> String {
    let name = child_name(child_thread_id, title);
    format!(
        "- CHILD STOPPED: the user stopped the turn of your child {name}. It is NOT finished \
         and NOT dead: it waits for the user. A [CHILD THREAD COMPLETED] block arrives when it \
         next finishes, or a canceled one if the user archives or discards it. Until then, do \
         not roll back its work, respawn it, or send it a follow-up."
    )
}

fn child_moved_out_line(child_thread_id: &str, title: &str) -> String {
    let name = child_name(child_thread_id, title);
    format!(
        "- CHILD MOVED OUT: your child {name} was moved to top level. It keeps running on its \
         own, but it is no longer your child: you will not get its result and cannot follow up \
         on it. Do not wait for it or respawn it."
    )
}

fn child_name(child_thread_id: &str, title: &str) -> String {
    if title.is_empty() {
        format!("thread {child_thread_id}")
    } else {
        format!("\"{title}\" (thread {child_thread_id})")
    }
}

fn discarded_line(label: &str, branch: &str, session_branch: Option<&str>) -> String {
    match (branch.is_empty(), session_branch) {
        (false, Some(current)) if branch != current => format!(
            "- DISCARDED: {} on branch `{}`, which is NOT your current branch `{}`. That branch \
             was reset to main; your current work is untouched.",
            label, branch, current
        ),
        (false, _) => format!(
            "- DISCARDED: {} on your branch `{}`. Discard reset that branch to main and cleaned \
             the worktree, so those commits are gone. It is NOT pending: do not tell the user it \
             is awaiting Apply, and do not offer to Apply it.",
            label, branch
        ),
        (true, _) => format!(
            "- DISCARDED: {}. Discard reset its branch to main and cleaned the worktree, so those \
             commits are gone. It is NOT pending: do not tell the user it is awaiting Apply, and \
             do not offer to Apply it.",
            label
        ),
    }
}

struct BackgroundTaskLine<'a> {
    command: &'a str,
    task_id: &'a str,
    abandoned: bool,
    timed_out: bool,
    killed: bool,
    status: &'a str,
    started_at: &'a str,
}

fn background_task_line(line: BackgroundTaskLine<'_>) -> String {
    let spawned = if line.started_at.is_empty() {
        String::new()
    } else {
        format!(", spawned {}", line.started_at)
    };
    if line.abandoned {
        return format!(
            "- BACKGROUND TASK LOST: `{}` (task {}{}) did NOT finish. The engine stopped while \
             it was running, so no exit status exists and nothing cancelled it. It may still be \
             running: drain it with bash_output, and check before starting the same work again. \
             Do not report the work as passed or failed.",
            line.command, line.task_id, spawned,
        );
    }
    let how = match (line.timed_out, line.killed) {
        (true, _) => " after blowing its own timeout",
        (_, true) => " because bash_kill stopped it",
        _ => "",
    };
    let status = if line.status.is_empty() {
        String::new()
    } else {
        format!(" with {}", line.status)
    };
    format!(
        "- BACKGROUND TASK FINISHED: `{}` (task {}{}) ended{}{}, and the wake never reached \
         you. Drain it with bash_output before saying anything about it.",
        line.command, line.task_id, spawned, status, how,
    )
}

fn worktree_cleaned_line(tier: u8, freed_bytes: u64, branch_deleted: bool) -> String {
    let freed = format_bytes(freed_bytes);
    if tier != 1 {
        let branch = if branch_deleted {
            " Its branch was deleted as fully merged."
        } else {
            ""
        };
        format!(
            "- WORKTREE CLEANED: Lucidos removed your whole worktree ({} reclaimed), and it has \
             been recreated. When it went because the thread was archived, uncommitted work was \
             committed on your branch first, as \"{}\". Ignored files, such as build output, \
             are gone.{}",
            freed,
            crate::engine::worktree_cleanup::SNAPSHOT_COMMIT_MESSAGE,
            branch
        )
    } else {
        format!(
            "- WORKTREE CLEANED: the cleanup worker reclaimed build artifacts from your worktree \
             ({} from `target/`, `node_modules/`, `.lucidos/cache/`). Commits and tracked files \
             are untouched, but the next build starts cold.",
            freed
        )
    }
}

fn closing_guidance(events: &[GapEvent]) -> Vec<&'static str> {
    let mut out = Vec::new();
    if events.iter().any(|e| matches!(e, GapEvent::Applied { .. })) {
        out.push(
            "Treat main as already containing the APPLIED work and your worktree as reset to \
             match main; build on it. Applied work is NOT pending: do not tell the user it is \
             awaiting Apply, and do not re-propose it.",
        );
    }
    if events
        .iter()
        .any(|e| matches!(e, GapEvent::Discarded { .. } | GapEvent::Reverted { .. }))
    {
        out.push(
            "Do not describe discarded or reverted work as pending, and do not offer to Apply it.",
        );
    }
    out
}

fn change_label(change_id: &str, facts: Option<&ChangeFacts>) -> String {
    match facts
        .map(|f| f.description.trim())
        .filter(|d| !d.is_empty())
    {
        Some(description) => format!("\"{}\"", truncate(description, MAX_DESCRIPTION_CHARS)),
        None => format!("change {}", short_change_id(change_id)),
    }
}

fn on_branch(branch: &str) -> String {
    if branch.is_empty() {
        String::new()
    } else {
        format!(" (branch `{}`)", branch)
    }
}

fn short_change_id(id: &str) -> String {
    if id.is_empty() {
        "?".to_string()
    } else {
        short(id)
    }
}

fn format_bytes(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else {
        format!("{} MB", bytes / MB)
    }
}

#[cfg(test)]
#[path = "turn_gap_tests.rs"]
mod tests;
