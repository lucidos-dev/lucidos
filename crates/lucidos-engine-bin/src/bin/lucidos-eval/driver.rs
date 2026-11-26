use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::config::{Task, TaskAnswers};
use crate::results::{EMPTY_STATUS, IDLE_STATUS, PARKED_STATUS, WOKEN_STATUS};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

const POLL_INTERVAL: Duration = Duration::from_secs(2);

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(60);

pub const UNSCRIPTED_REPLY: &str =
    "Stick to the original request and leave the rest. Mention anything further at the end of \
     your reply.";

pub const EVAL_DEVICE_ID: &str = "lucidos-eval-driver";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Idle,
    IdleAfterWake,
    Empty,
    Parked,
    Timeout,
    Settled,
}

impl TaskStatus {
    pub fn completed(self) -> bool {
        matches!(self, TaskStatus::Idle | TaskStatus::IdleAfterWake)
    }

    fn severity(self) -> u8 {
        match self {
            TaskStatus::Idle => 0,
            TaskStatus::IdleAfterWake => 1,
            TaskStatus::Empty => 2,
            TaskStatus::Settled => 3,
            TaskStatus::Parked => 4,
            TaskStatus::Timeout => 5,
        }
    }
}

pub fn worse_of(first: TaskStatus, second: TaskStatus) -> TaskStatus {
    match second.severity() > first.severity() {
        true => second,
        false => first,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrivenTask {
    pub task: String,
    pub thread_id: Uuid,
    pub status: TaskStatus,
    pub terminal_status: String,
    pub unscripted_answers: u32,
    pub questions_answered: u32,
    pub empty_completions: u32,
    pub empty_retries: u32,
    pub refusals: u32,
    pub refusal_retries: u32,
    pub followup_sequence: Option<i64>,
}

impl DrivenTask {
    pub fn completed(&self) -> bool {
        self.status.completed()
    }

    fn record_fault(&mut self, fault: TurnFault) {
        match fault {
            TurnFault::Empty => self.empty_completions += 1,
            TurnFault::Refused => self.refusals += 1,
        }
    }

    fn record_repost(&mut self, fault: TurnFault) {
        match fault {
            TurnFault::Empty => self.empty_retries += 1,
            TurnFault::Refused => self.refusal_retries += 1,
        }
    }

    pub fn row_status(&self) -> String {
        match self.status {
            TaskStatus::Timeout => "timeout".to_string(),
            TaskStatus::Parked => PARKED_STATUS.to_string(),
            TaskStatus::IdleAfterWake => WOKEN_STATUS.to_string(),
            TaskStatus::Empty => EMPTY_STATUS.to_string(),
            TaskStatus::Idle | TaskStatus::Settled => self.terminal_status.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadState {
    pub status: String,
    pub live_event_waits: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Poll,
    Answer,
    Stop(TaskStatus),
}

const QUIET_POLLS_AFTER_WAKE: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveProgress {
    parked: bool,
    quiet_polls: u32,
    quiet_polls_needed: u32,
}

impl Default for DriveProgress {
    fn default() -> Self {
        DriveProgress {
            parked: false,
            quiet_polls: 0,
            quiet_polls_needed: 1,
        }
    }
}

impl DriveProgress {
    pub fn after_prompt() -> Self {
        DriveProgress {
            quiet_polls_needed: QUIET_POLLS_AFTER_WAKE,
            ..DriveProgress::default()
        }
    }

    pub fn observe(&mut self, state: &ThreadState, past_deadline: bool) -> Step {
        if state.live_event_waits > 0 {
            self.parked = true;
            self.quiet_polls = 0;
            self.quiet_polls_needed = QUIET_POLLS_AFTER_WAKE;
        } else if state.status == IDLE_STATUS {
            self.quiet_polls += 1;
        } else {
            self.quiet_polls = 0;
        }
        match self.classify(state) {
            stop @ Step::Stop(_) => stop,
            _ if past_deadline => Step::Stop(self.at_deadline(state)),
            step => step,
        }
    }

    fn classify(&self, state: &ThreadState) -> Step {
        match state.status.as_str() {
            "failed" | "paused" => Step::Stop(TaskStatus::Settled),
            "waiting_for_user_answer" => Step::Answer,
            IDLE_STATUS if state.live_event_waits == 0 && self.quiet_enough() => {
                Step::Stop(match self.parked {
                    true => TaskStatus::IdleAfterWake,
                    false => TaskStatus::Idle,
                })
            }
            _ => Step::Poll,
        }
    }

    fn quiet_enough(&self) -> bool {
        self.quiet_polls >= self.quiet_polls_needed
    }

    fn at_deadline(&self, state: &ThreadState) -> TaskStatus {
        match state.live_event_waits > 0 {
            true => TaskStatus::Parked,
            false => TaskStatus::Timeout,
        }
    }
}

pub const REPOSTS_PER_TURN: u32 = 2;

const NO_OUTPUT_REFUSAL: &str = "Model returned no response (stop_reason: refusal,";

pub fn is_no_output_refusal(error: &str) -> bool {
    error.starts_with(NO_OUTPUT_REFUSAL)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnOutput {
    pub said_something: bool,
    pub tool_calls: i64,
    pub input_total: i64,
    pub refused: bool,
}

impl TurnOutput {
    pub fn produced_nothing(&self) -> bool {
        !self.said_something && self.tool_calls == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnFault {
    Empty,
    Refused,
}

impl TurnFault {
    fn of(status: TaskStatus, output: &TurnOutput) -> Option<TurnFault> {
        match status {
            TaskStatus::Idle | TaskStatus::IdleAfterWake if output.produced_nothing() => {
                Some(TurnFault::Empty)
            }
            TaskStatus::Settled if output.refused => Some(TurnFault::Refused),
            _ => None,
        }
    }

    pub fn given_up(self) -> TaskStatus {
        match self {
            TurnFault::Empty => TaskStatus::Empty,
            TurnFault::Refused => TaskStatus::Settled,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStep {
    Keep(TaskStatus),
    Retry(TurnFault),
    GiveUp(TurnFault),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepostPolicy {
    retries_used: u32,
}

impl RepostPolicy {
    pub fn observe(&mut self, status: TaskStatus, output: &TurnOutput) -> TurnStep {
        let Some(fault) = TurnFault::of(status, output) else {
            return TurnStep::Keep(status);
        };
        if self.retries_used >= REPOSTS_PER_TURN {
            return TurnStep::GiveUp(fault);
        }
        self.retries_used += 1;
        TurnStep::Retry(fault)
    }

    pub fn retries_used(&self) -> u32 {
        self.retries_used
    }
}

const QUIET_OBSERVATIONS_FOR_SETTLED: u32 = 10;

const SETTLE_INTERVAL: Duration = Duration::from_secs(1);

const SETTLE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleStep {
    Poll,
    Ready,
    GaveUp,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settling {
    highest: Option<i64>,
    quiet: u32,
}

impl Settling {
    pub fn observe(&mut self, highest: i64, past_deadline: bool) -> SettleStep {
        match self.highest {
            Some(mark) if mark == highest => self.quiet += 1,
            _ => {
                self.highest = Some(highest);
                self.quiet = 0;
            }
        }
        match (self.quiet >= QUIET_OBSERVATIONS_FOR_SETTLED, past_deadline) {
            (true, _) => SettleStep::Ready,
            (false, true) => SettleStep::GaveUp,
            (false, false) => SettleStep::Poll,
        }
    }
}

pub async fn wait_until_settled(pool: &PgPool, thread_id: Uuid) -> Fallible<bool> {
    let deadline = Instant::now() + SETTLE_TIMEOUT;
    let mut settling = Settling::default();
    loop {
        let highest = crate::metrics::highest_sequence(pool, thread_id).await?;
        match settling.observe(highest, Instant::now() >= deadline) {
            SettleStep::Ready => return Ok(true),
            SettleStep::GaveUp => return Ok(false),
            SettleStep::Poll => tokio::time::sleep(SETTLE_INTERVAL).await,
        }
    }
}

const TREE_READY_TIMEOUT: Duration = Duration::from_secs(600);

const COMPACTION_TIMEOUT: Duration = Duration::from_secs(600);

const TREE_POLL_INTERVAL: Duration = Duration::from_secs(2);

pub async fn wait_until_tree_ready(pool: &PgPool) -> bool {
    let deadline = Instant::now() + TREE_READY_TIMEOUT;
    loop {
        if lucidos_engine::engine::summary_tree::tree_ready(pool).await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(TREE_POLL_INTERVAL).await;
    }
}

pub async fn wait_until_compacted(pool: &PgPool) -> Fallible<bool> {
    let deadline = Instant::now() + COMPACTION_TIMEOUT;
    loop {
        if lucidos_engine::engine::summary_tree::caught_up(pool).await? {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        tokio::time::sleep(TREE_POLL_INTERVAL).await;
    }
}

pub struct ArmEndpoint {
    pub base_url: String,
    pub client: reqwest::Client,
    pub pool: PgPool,
}

impl ArmEndpoint {
    pub async fn connect(base_url: &str, pool: PgPool) -> Fallible<ArmEndpoint> {
        let bootstrap = lucidos_local_token::client::client_builder().build()?;
        bootstrap
            .post(format!("{base_url}/api/v1/devices/register"))
            .json(&serde_json::json!({
                "device_id": EVAL_DEVICE_ID,
                "user_agent": "lucidos-eval/1",
            }))
            .send()
            .await?
            .error_for_status()?;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            "x-lucidos-device-id",
            reqwest::header::HeaderValue::from_static(EVAL_DEVICE_ID),
        );
        Ok(ArmEndpoint {
            base_url: base_url.to_string(),
            client: lucidos_local_token::client::client_builder()
                .default_headers(headers)
                .build()?,
            pool,
        })
    }
}

pub async fn drive_task(
    arm: &ArmEndpoint,
    task: &Task,
    marker: &str,
    scope_rule: &str,
    answers: Option<&TaskAnswers>,
) -> Fallible<DrivenTask> {
    let prompt = task.rendered_prompt(marker, scope_rule);
    post_message(arm, &prompt, None).await?;

    let thread_id = resolve_thread_by_marker(&arm.pool, marker).await?;
    let mut driven = DrivenTask {
        task: task.id.clone(),
        thread_id,
        status: TaskStatus::Timeout,
        terminal_status: String::new(),
        unscripted_answers: 0,
        questions_answered: 0,
        empty_completions: 0,
        empty_retries: 0,
        refusals: 0,
        refusal_retries: 0,
        followup_sequence: None,
    };
    let mut turns = Turns {
        prompts_posted: 1,
        scripted_used: 0,
        deadline: Instant::now() + task.timeout(),
        answers,
    };
    let first = drive_one_turn(
        arm,
        &mut driven,
        &prompt,
        DriveProgress::default(),
        &mut turns,
    )
    .await?;
    driven.status = first.status;

    let Some(followup) = followup_to_send(task, driven.status, marker, scope_rule) else {
        return Ok(driven);
    };
    post_message(arm, &followup, Some(thread_id)).await?;
    turns.prompts_posted += 1;
    let second = drive_one_turn(
        arm,
        &mut driven,
        &followup,
        DriveProgress::after_prompt(),
        &mut turns,
    )
    .await?;
    driven.followup_sequence = Some(second.opened_at);
    driven.status = worse_of(driven.status, second.status);
    Ok(driven)
}

struct Turns<'a> {
    prompts_posted: usize,
    scripted_used: usize,
    deadline: Instant,
    answers: Option<&'a TaskAnswers>,
}

struct DrivenTurn {
    status: TaskStatus,
    opened_at: i64,
}

async fn drive_one_turn(
    arm: &ArmEndpoint,
    driven: &mut DrivenTask,
    prompt: &str,
    mut progress: DriveProgress,
    turns: &mut Turns<'_>,
) -> Fallible<DrivenTurn> {
    let mut policy = RepostPolicy::default();
    let opened_at = await_prompt(
        &arm.pool,
        driven.thread_id,
        turns.prompts_posted,
        &driven.task,
    )
    .await?;
    let mut attempt_opened_at = opened_at;
    loop {
        let status = drive_turn(arm, driven, progress, turns).await?;
        let output = turn_output(&arm.pool, driven.thread_id, attempt_opened_at).await?;
        let step = policy.observe(status, &output);
        let fault = match step {
            TurnStep::Keep(status) => return Ok(DrivenTurn { status, opened_at }),
            TurnStep::Retry(fault) | TurnStep::GiveUp(fault) => fault,
        };
        driven.record_fault(fault);
        report_fault(&driven.task, &output, step, policy.retries_used());
        if let TurnStep::GiveUp(fault) = step {
            return Ok(DrivenTurn {
                status: fault.given_up(),
                opened_at,
            });
        }
        post_message(arm, prompt, Some(driven.thread_id)).await?;
        turns.prompts_posted += 1;
        driven.record_repost(fault);
        attempt_opened_at = await_prompt(
            &arm.pool,
            driven.thread_id,
            turns.prompts_posted,
            &driven.task,
        )
        .await?;
        progress = DriveProgress::after_prompt();
    }
}

fn report_fault(task: &str, output: &TurnOutput, step: TurnStep, attempt: u32) {
    let next = match step {
        TurnStep::GiveUp(TurnFault::Empty) => {
            "giving up, and the task is voided in both arms".to_string()
        }
        TurnStep::GiveUp(TurnFault::Refused) => {
            "giving up, and the thread counts as failed".to_string()
        }
        _ => format!("re-posting the prompt, attempt {attempt} of {REPOSTS_PER_TURN}"),
    };
    let what = match step {
        TurnStep::Retry(TurnFault::Refused) | TurnStep::GiveUp(TurnFault::Refused) => {
            "on a provider refusal with no output"
        }
        _ => "with no text and no tool call",
    };
    println!(
        "[eval]   {task} ended a turn {what} ({} input tokens): {next}",
        output.input_total
    );
}

pub fn followup_to_send(
    task: &Task,
    first_turn: TaskStatus,
    marker: &str,
    scope_rule: &str,
) -> Option<String> {
    match first_turn.completed() {
        true => task.rendered_followup(marker, scope_rule),
        false => None,
    }
}

async fn post_message(arm: &ArmEndpoint, message: &str, thread: Option<Uuid>) -> Fallible<()> {
    arm.client
        .post(format!("{}/api/v1/chat/stream", arm.base_url))
        .json(&chat_body(message, thread))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

fn chat_body(message: &str, thread: Option<Uuid>) -> serde_json::Value {
    serde_json::json!({
        "message": message,
        "mode": "human",
        "thread_id": thread.map(|id| id.to_string()),
    })
}

async fn drive_turn(
    arm: &ArmEndpoint,
    driven: &mut DrivenTask,
    mut progress: DriveProgress,
    turns: &mut Turns<'_>,
) -> Fallible<TaskStatus> {
    let thread_id = driven.thread_id;
    loop {
        let state = thread_state(&arm.pool, thread_id).await?;
        driven.terminal_status = state.status.clone();
        match progress.observe(&state, Instant::now() >= turns.deadline) {
            Step::Stop(status) => return Ok(status),
            Step::Answer => {
                if let Some(question) = pending_question(&arm.pool, thread_id).await? {
                    let reply = choose_reply(&question, turns.answers, &mut turns.scripted_used);
                    if reply.unscripted {
                        driven.unscripted_answers += 1;
                    }
                    answer_question(arm, thread_id, &question, &reply).await?;
                    driven.questions_answered += 1;
                }
            }
            Step::Poll => {}
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn await_prompt(pool: &PgPool, thread_id: Uuid, nth: usize, task: &str) -> Fallible<i64> {
    let deadline = Instant::now() + RESOLVE_TIMEOUT;
    loop {
        if let Some(sequence) = crate::metrics::prompt_sequence(pool, thread_id, nth).await? {
            return Ok(sequence);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "prompt_not_delivered: prompt {nth} of {task} was accepted, but no matching \
                 MessageReceived landed on its thread within {}s. This is a harness failure \
                 and not a result.",
                RESOLVE_TIMEOUT.as_secs()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn turn_output(pool: &PgPool, thread_id: Uuid, opened_at: i64) -> Fallible<TurnOutput> {
    let row = sqlx::query(
        "SELECT \
           EXISTS (SELECT 1 FROM events \
                    WHERE thread_id = $1 AND sequence > $2 \
                      AND event_type = 'ResponseGenerated' \
                      AND COALESCE(btrim(payload->>'text'), '') <> '') AS said_something, \
           (SELECT count(*) FROM events \
             WHERE thread_id = $1 AND sequence > $2 \
               AND event_type = 'ToolCalled') AS tool_calls, \
           COALESCE((SELECT sum((payload->'usage'->>'input_tokens')::bigint) FROM events \
                      WHERE thread_id = $1 AND sequence > $2 \
                        AND event_type = 'ContextCaptured'), 0)::bigint AS input_total, \
           ARRAY(SELECT COALESCE(payload->>'error', '') FROM events \
                  WHERE thread_id = $1 AND sequence > $2 \
                    AND event_type = 'ResponseFailed') AS failures",
    )
    .bind(thread_id)
    .bind(opened_at)
    .fetch_one(pool)
    .await?;
    let failures: Vec<String> = row.try_get("failures")?;
    Ok(TurnOutput {
        said_something: row.try_get("said_something")?,
        tool_calls: row.try_get("tool_calls")?,
        input_total: row.try_get("input_total")?,
        refused: failures.iter().any(|error| is_no_output_refusal(error)),
    })
}

pub async fn resolve_thread_by_marker(pool: &PgPool, marker: &str) -> Fallible<Uuid> {
    let deadline = Instant::now() + RESOLVE_TIMEOUT;
    loop {
        if let Some(thread_id) = find_thread_by_marker(pool, marker).await? {
            return Ok(thread_id);
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "thread_not_resolved: no thread carrying marker {marker} appeared within \
                 {}s. The prompt was accepted, so this is a harness failure and not a result.",
                RESOLVE_TIMEOUT.as_secs()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

pub async fn find_thread_by_marker(pool: &PgPool, marker: &str) -> Fallible<Option<Uuid>> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        "SELECT thread_id FROM thread_summaries WHERE first_message LIKE $1 LIMIT 1",
    )
    .bind(format!("%{marker}%"))
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(thread_id,)| thread_id))
}

pub async fn thread_exists(pool: &PgPool, thread_id: Uuid) -> Fallible<bool> {
    let row: Option<(Uuid,)> =
        sqlx::query_as("SELECT thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

async fn thread_state(pool: &PgPool, thread_id: Uuid) -> Fallible<ThreadState> {
    let row: Option<(String, i32)> = sqlx::query_as(
        "SELECT status, live_event_wait_count FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let (status, live_event_waits) = row.unwrap_or_default();
    Ok(ThreadState {
        status,
        live_event_waits: live_event_waits as i64,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct PendingQuestion {
    pub tool_use_id: String,
    pub text: String,
    pub options: Vec<(String, String)>,
}

pub async fn pending_question(pool: &PgPool, thread_id: Uuid) -> Fallible<Option<PendingQuestion>> {
    let row = sqlx::query(
        "SELECT payload->>'tool_use_id' AS tool_use_id, \
                payload->>'question'    AS question, \
                COALESCE(payload->'options', '[]'::jsonb)::text AS options \
           FROM events asked \
          WHERE asked.thread_id = $1 AND asked.event_type = 'UserQuestionAsked' \
            AND NOT EXISTS ( \
              SELECT 1 FROM events answered \
               WHERE answered.thread_id = $1 \
                 AND answered.event_type = 'UserQuestionAnswered' \
                 AND answered.payload->>'tool_use_id' = asked.payload->>'tool_use_id') \
          ORDER BY asked.sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let options: Vec<serde_json::Value> =
        serde_json::from_str(&row.try_get::<String, _>("options")?)?;
    Ok(Some(PendingQuestion {
        tool_use_id: row
            .try_get::<Option<String>, _>("tool_use_id")?
            .unwrap_or_default(),
        text: row
            .try_get::<Option<String>, _>("question")?
            .unwrap_or_default(),
        options: options
            .iter()
            .filter_map(|option| {
                Some((
                    option.get("id")?.as_str()?.to_string(),
                    option.get("label")?.as_str()?.to_string(),
                ))
            })
            .collect(),
    }))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: String,
    pub unscripted: bool,
}

pub fn choose_reply(
    question: &PendingQuestion,
    answers: Option<&TaskAnswers>,
    scripted_used: &mut usize,
) -> Reply {
    if let Some(answers) = answers {
        for scripted in answers.scripted.iter().skip(*scripted_used) {
            let matches = Regex::new(&scripted.matches)
                .map(|r| r.is_match(&question.text))
                .unwrap_or(false);
            if matches {
                *scripted_used += 1;
                return Reply {
                    text: scripted.answer.clone(),
                    unscripted: false,
                };
            }
        }
    }
    Reply {
        text: UNSCRIPTED_REPLY.to_string(),
        unscripted: true,
    }
}

pub fn answer_body(question: &PendingQuestion, reply: &Reply) -> serde_json::Value {
    let wanted = reply.text.trim().to_ascii_lowercase();
    let selected = match reply.unscripted {
        true => None,
        false => question
            .options
            .iter()
            .find(|(_, label)| label.trim().to_ascii_lowercase() == wanted)
            .or_else(|| {
                question.options.iter().find(|(_, label)| {
                    label.to_ascii_lowercase().contains(&wanted)
                        || wanted.contains(&label.to_ascii_lowercase())
                })
            }),
    };
    let answer = match selected {
        Some((id, _)) => serde_json::json!({ "kind": "Selected", "option_id": id }),
        None => serde_json::json!({ "kind": "FreeText", "text": reply.text }),
    };
    serde_json::json!({ "tool_use_id": question.tool_use_id, "answer": answer })
}

async fn answer_question(
    arm: &ArmEndpoint,
    thread_id: Uuid,
    question: &PendingQuestion,
    reply: &Reply,
) -> Fallible<()> {
    arm.client
        .post(format!(
            "{}/api/v1/threads/{thread_id}/answer-question",
            arm.base_url
        ))
        .json(&answer_body(question, reply))
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

pub async fn final_response(pool: &PgPool, thread_id: Uuid) -> Fallible<String> {
    let text: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'text' FROM events \
          WHERE thread_id = $1 AND event_type = 'ResponseGenerated' \
          ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?
    .flatten();
    Ok(text.unwrap_or_default())
}

pub fn task_marker(run_id: &str, repeat: u32, arm: crate::arm::Arm, task: &str) -> String {
    format!("eval-{run_id}-r{repeat}-{arm}-{task}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ScriptedAnswer;

    fn question(text: &str, options: &[(&str, &str)]) -> PendingQuestion {
        PendingQuestion {
            tool_use_id: "toolu_1".into(),
            text: text.into(),
            options: options
                .iter()
                .map(|(id, label)| (id.to_string(), label.to_string()))
                .collect(),
        }
    }

    fn answers() -> TaskAnswers {
        TaskAnswers {
            task: "T09".into(),
            scripted: vec![ScriptedAnswer {
                matches: "(?i)group".into(),
                answer: "by project".into(),
            }],
        }
    }

    #[test]
    fn a_matching_question_gets_its_scripted_answer() {
        let mut used = 0;
        let reply = choose_reply(
            &question("How should I group the breakdown?", &[]),
            Some(&answers()),
            &mut used,
        );
        assert_eq!(reply.text, "by project");
        assert!(!reply.unscripted);
        assert_eq!(used, 1);
    }

    #[test]
    fn a_second_question_past_the_script_falls_back_and_is_counted() {
        let mut used = 1;
        let reply = choose_reply(
            &question("How should I group the breakdown?", &[]),
            Some(&answers()),
            &mut used,
        );
        assert_eq!(reply.text, UNSCRIPTED_REPLY);
        assert!(reply.unscripted);
    }

    #[test]
    fn a_task_with_no_script_always_falls_back() {
        let mut used = 0;
        let reply = choose_reply(&question("Anything?", &[]), None, &mut used);
        assert!(reply.unscripted);
    }

    #[test]
    fn a_reply_matching_an_offered_label_is_sent_as_that_option() {
        let question = question(
            "How should I group it?",
            &[("opt_a", "By project"), ("opt_b", "By day")],
        );
        let body = answer_body(
            &question,
            &Reply {
                text: "by project".into(),
                unscripted: false,
            },
        );
        assert_eq!(body["answer"]["kind"], "Selected");
        assert_eq!(body["answer"]["option_id"], "opt_a");
        assert_eq!(body["tool_use_id"], "toolu_1");
    }

    #[test]
    fn the_unscripted_reply_sends_the_agent_back_to_the_request() {
        let lowered = UNSCRIPTED_REPLY.to_ascii_lowercase();
        assert!(!lowered.contains("judgment") && !lowered.contains("judgement"));
        assert!(lowered.contains("original request"));
    }

    #[test]
    fn an_unscripted_reply_is_free_text_even_when_a_label_hides_inside_it() {
        let colliding = question(
            "Should I also do the other thing?",
            &[("opt_a", "No"), ("opt_b", "Stick")],
        );
        let body = answer_body(
            &colliding,
            &Reply {
                text: UNSCRIPTED_REPLY.into(),
                unscripted: true,
            },
        );
        assert_eq!(body["answer"]["kind"], "FreeText");
        assert_eq!(body["answer"]["text"], UNSCRIPTED_REPLY);
    }

    #[test]
    fn an_unscripted_reply_exactly_equal_to_a_label_is_still_free_text() {
        let body = answer_body(
            &question("Which one?", &[("opt_a", "Red")]),
            &Reply {
                text: "Red".into(),
                unscripted: true,
            },
        );
        assert_eq!(body["answer"]["kind"], "FreeText");
        let scripted = answer_body(
            &question("Which one?", &[("opt_a", "Red")]),
            &Reply {
                text: "Red".into(),
                unscripted: false,
            },
        );
        assert_eq!(scripted["answer"]["kind"], "Selected");
    }

    #[test]
    fn a_reply_matching_no_option_is_sent_as_free_text() {
        let question = question("Which one?", &[("opt_a", "Red"), ("opt_b", "Blue")]);
        let body = answer_body(
            &question,
            &Reply {
                text: UNSCRIPTED_REPLY.into(),
                unscripted: true,
            },
        );
        assert_eq!(body["answer"]["kind"], "FreeText");
        assert_eq!(body["answer"]["text"], UNSCRIPTED_REPLY);
    }

    #[test]
    fn a_question_with_no_options_is_always_free_text() {
        let body = answer_body(
            &question("Open question?", &[]),
            &Reply {
                text: "by project".into(),
                unscripted: false,
            },
        );
        assert_eq!(body["answer"]["kind"], "FreeText");
    }

    #[test]
    fn a_marker_is_unique_per_arm_repeat_and_task() {
        let control = task_marker("abc", 1, crate::arm::Arm::CONTROL, "T05");
        let lean = task_marker("abc", 1, crate::arm::Arm::LEAN, "T05");
        assert_ne!(control, lean);
        assert!(control.contains("T05"));
        assert!(control.starts_with("eval-abc-r1-"));
    }

    fn driven(status: TaskStatus, terminal: &str) -> DrivenTask {
        DrivenTask {
            task: "T08".into(),
            thread_id: Uuid::nil(),
            status,
            terminal_status: terminal.into(),
            unscripted_answers: 0,
            questions_answered: 0,
            empty_completions: 0,
            empty_retries: 0,
            refusals: 0,
            refusal_retries: 0,
            followup_sequence: None,
        }
    }

    #[test]
    fn a_task_counts_as_completed_when_it_reached_an_end_of_its_own() {
        for status in [TaskStatus::Idle, TaskStatus::IdleAfterWake] {
            assert!(driven(status, "idle").completed(), "{status:?}");
        }
        for status in [
            TaskStatus::Timeout,
            TaskStatus::Settled,
            TaskStatus::Parked,
            TaskStatus::Empty,
        ] {
            assert!(!driven(status, "idle").completed(), "{status:?}");
        }
    }

    #[test]
    fn a_park_and_a_wake_are_written_under_their_own_names() {
        assert_eq!(driven(TaskStatus::Parked, "idle").row_status(), "parked");
        assert_eq!(
            driven(TaskStatus::IdleAfterWake, "idle").row_status(),
            "idle-after-wake"
        );
        assert_eq!(
            driven(TaskStatus::Timeout, "running").row_status(),
            "timeout"
        );
        assert_eq!(driven(TaskStatus::Idle, "idle").row_status(), "idle");
        assert_eq!(driven(TaskStatus::Settled, "failed").row_status(), "failed");
    }

    fn state(status: &str, live_event_waits: i64) -> ThreadState {
        ThreadState {
            status: status.into(),
            live_event_waits,
        }
    }

    #[test]
    fn an_idle_thread_holding_an_event_wait_is_not_finished() {
        let mut progress = DriveProgress::default();
        assert_eq!(progress.observe(&state("idle", 1), false), Step::Poll);
    }

    #[test]
    fn a_thread_that_never_parked_finishes_on_the_first_quiet_poll() {
        let mut progress = DriveProgress::default();
        assert_eq!(
            progress.observe(&state("idle", 0), false),
            Step::Stop(TaskStatus::Idle)
        );
    }

    #[test]
    fn a_parked_thread_is_driven_through_its_wake_and_recorded_as_woken() {
        let mut progress = DriveProgress::default();
        assert_eq!(progress.observe(&state("idle", 1), false), Step::Poll);
        assert_eq!(progress.observe(&state("idle", 1), false), Step::Poll);
        assert_eq!(progress.observe(&state("running", 0), false), Step::Poll);
        assert_eq!(progress.observe(&state("idle", 0), false), Step::Poll);
        assert_eq!(
            progress.observe(&state("idle", 0), false),
            Step::Stop(TaskStatus::IdleAfterWake)
        );
    }

    #[test]
    fn a_second_wait_restarts_the_quiet_run() {
        let mut progress = DriveProgress::default();
        assert_eq!(progress.observe(&state("idle", 1), false), Step::Poll);
        assert_eq!(progress.observe(&state("idle", 0), false), Step::Poll);
        assert_eq!(progress.observe(&state("idle", 1), false), Step::Poll);
        assert_eq!(progress.observe(&state("idle", 0), false), Step::Poll);
        assert_eq!(
            progress.observe(&state("idle", 0), false),
            Step::Stop(TaskStatus::IdleAfterWake)
        );
    }

    #[test]
    fn a_wait_still_open_at_the_deadline_is_a_park_and_never_a_timeout() {
        let mut progress = DriveProgress::default();
        assert_eq!(
            progress.observe(&state("idle", 1), true),
            Step::Stop(TaskStatus::Parked)
        );
    }

    #[test]
    fn a_deadline_reached_after_the_wake_is_an_ordinary_timeout() {
        let mut progress = DriveProgress::default();
        assert_eq!(progress.observe(&state("idle", 1), false), Step::Poll);
        assert_eq!(
            progress.observe(&state("running", 0), true),
            Step::Stop(TaskStatus::Timeout)
        );
    }

    #[test]
    fn a_thread_that_finished_on_the_deadline_poll_is_not_a_timeout() {
        let mut progress = DriveProgress::default();
        assert_eq!(
            progress.observe(&state("idle", 0), true),
            Step::Stop(TaskStatus::Idle)
        );
    }

    #[test]
    fn a_failed_thread_settles_even_while_it_holds_a_wait() {
        for status in ["failed", "paused"] {
            let mut progress = DriveProgress::default();
            assert_eq!(
                progress.observe(&state(status, 1), false),
                Step::Stop(TaskStatus::Settled)
            );
        }
    }

    #[test]
    fn a_pending_question_is_answered_even_while_a_wait_is_open() {
        let mut progress = DriveProgress::default();
        assert_eq!(
            progress.observe(&state("waiting_for_user_answer", 1), false),
            Step::Answer
        );
    }

    fn task(followup: Option<&str>) -> Task {
        Task {
            id: "T14".into(),
            title: "T14".into(),
            prompt: "{marker} read the five documents".into(),
            timeout_minutes: 60,
            followup: followup.map(str::to_string),
            depends_on: vec![],
            preconditions: vec![],
        }
    }

    #[test]
    fn a_task_with_no_followup_sends_one_prompt_and_stops() {
        assert_eq!(
            followup_to_send(&task(None), TaskStatus::Idle, "eval-7f", "stay in scope"),
            None
        );
    }

    #[test]
    fn a_followup_is_sent_once_turn_one_finished() {
        let sent = followup_to_send(
            &task(Some("{marker} now tell me the hold time")),
            TaskStatus::Idle,
            "eval-7f",
            "stay in scope",
        );
        assert_eq!(
            sent.as_deref(),
            Some("eval-7f now tell me the hold time\n\nstay in scope")
        );
        assert!(followup_to_send(
            &task(Some("more")),
            TaskStatus::IdleAfterWake,
            "eval-7f",
            "stay in scope"
        )
        .is_some());
    }

    #[test]
    fn a_first_turn_that_did_not_finish_suppresses_the_followup() {
        for status in [TaskStatus::Timeout, TaskStatus::Parked, TaskStatus::Settled] {
            assert_eq!(
                followup_to_send(&task(Some("more")), status, "eval-7f", "stay in scope"),
                None,
                "{status:?}"
            );
        }
    }

    #[test]
    fn a_followup_names_the_thread_it_belongs_to() {
        let thread = Uuid::from_u128(0x5eed);
        let second = chat_body("now tell me the hold time", Some(thread));
        assert_eq!(second["thread_id"], thread.to_string());

        let first = chat_body("read the five documents", None);
        assert!(first["thread_id"].is_null());
        assert_eq!(first["mode"], "human");
    }

    #[test]
    fn a_turn_opened_by_a_prompt_needs_the_quiet_run_a_wake_needs() {
        let mut progress = DriveProgress::after_prompt();
        assert_eq!(progress.observe(&state("idle", 0), false), Step::Poll);
        assert_eq!(
            progress.observe(&state("idle", 0), false),
            Step::Stop(TaskStatus::Idle)
        );
    }

    #[test]
    fn a_clean_turn_two_is_not_recorded_as_a_wake() {
        let mut progress = DriveProgress::after_prompt();
        progress.observe(&state("idle", 0), false);
        assert_eq!(
            progress.observe(&state("idle", 0), false),
            Step::Stop(TaskStatus::Idle)
        );
        assert_eq!(
            worse_of(TaskStatus::IdleAfterWake, TaskStatus::Idle),
            TaskStatus::IdleAfterWake
        );
    }

    #[test]
    fn the_recorded_status_is_the_worse_of_the_two_turns() {
        assert_eq!(
            worse_of(TaskStatus::Idle, TaskStatus::Timeout),
            TaskStatus::Timeout
        );
        assert_eq!(
            worse_of(TaskStatus::Idle, TaskStatus::Parked),
            TaskStatus::Parked
        );
        assert_eq!(
            worse_of(TaskStatus::Settled, TaskStatus::Idle),
            TaskStatus::Settled
        );
        assert_eq!(
            worse_of(TaskStatus::Idle, TaskStatus::Idle),
            TaskStatus::Idle
        );
        assert!(!worse_of(TaskStatus::IdleAfterWake, TaskStatus::Timeout).completed());
    }

    #[test]
    fn the_same_observations_drive_the_same_way_whichever_arm_made_them() {
        let observations = [state("idle", 1), state("running", 0), state("idle", 0)];
        let mut control = DriveProgress::default();
        let mut lean = DriveProgress::default();
        for observation in &observations {
            assert_eq!(
                control.observe(observation, false),
                lean.observe(observation, false)
            );
        }
        assert_eq!(control, lean);
    }

    fn nothing() -> TurnOutput {
        TurnOutput {
            said_something: false,
            tool_calls: 0,
            input_total: 0,
            refused: false,
        }
    }

    fn terse_answer() -> TurnOutput {
        TurnOutput {
            said_something: true,
            tool_calls: 0,
            input_total: 35_628,
            refused: false,
        }
    }

    fn work_then_silence() -> TurnOutput {
        TurnOutput {
            said_something: false,
            tool_calls: 9,
            input_total: 177_589,
            refused: false,
        }
    }

    fn work_then_refusal() -> TurnOutput {
        TurnOutput {
            refused: true,
            ..work_then_silence()
        }
    }

    const RUN_4_REFUSAL: &str = "Model returned no response (stop_reason: refusal, \
         output_tokens: 0, thinking_chars: 0, model: claude-opus-5-5 \u{2014} the model declined \
         to respond (provider safety classifier withheld the output). Rephrase the request or \
         start a new thread; retrying the same prompt will be refused again).";

    #[test]
    fn the_engine_failure_for_a_refusal_with_no_output_is_read_as_one() {
        assert!(is_no_output_refusal(RUN_4_REFUSAL));
    }

    #[test]
    fn the_refusal_prefix_is_the_text_the_engine_writes() {
        let run = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../lucidos-agents-api/src/engine/agentic_loop/run.rs"
        ));
        let failure = r#""Model returned no response ({}).""#;
        let diagnostic = r#""stop_reason: {}, output_tokens: {}"#;
        for fragment in [failure, diagnostic] {
            assert!(
                run.contains(fragment),
                "the engine no longer writes {fragment}"
            );
        }
        let prefix = failure.trim_matches('"').replace("{}).", "")
            + &diagnostic
                .trim_start_matches('"')
                .replace("{}, output_tokens: {}", "refusal,");
        assert_eq!(NO_OUTPUT_REFUSAL, prefix);
    }

    #[test]
    fn other_engine_failures_are_never_read_as_a_refusal() {
        for error in [
            "The model stopped partway: the provider's safety classifier withheld the rest of \
             the response.",
            "Model returned no response (stop_reason: max_tokens, output_tokens: 32000).",
            "Model returned no response (stop_reason: end_turn, output_tokens: 40, \
             thinking_chars: 0, model: claude-opus-5-5 \u{2014} model generated output the \
             engine couldn't classify).",
            "",
        ] {
            assert!(!is_no_output_refusal(error), "{error}");
        }
    }

    #[test]
    fn a_refused_turn_that_answers_on_the_retry_is_kept() {
        let mut policy = RepostPolicy::default();
        assert_eq!(
            policy.observe(TaskStatus::Settled, &work_then_refusal()),
            TurnStep::Retry(TurnFault::Refused)
        );
        assert_eq!(
            policy.observe(TaskStatus::Idle, &terse_answer()),
            TurnStep::Keep(TaskStatus::Idle)
        );
        assert_eq!(policy.retries_used(), 1);
    }

    #[test]
    fn a_turn_refused_every_time_gives_up_as_a_failure() {
        let mut policy = RepostPolicy::default();
        for _ in 0..REPOSTS_PER_TURN {
            assert_eq!(
                policy.observe(TaskStatus::Settled, &work_then_refusal()),
                TurnStep::Retry(TurnFault::Refused)
            );
        }
        assert_eq!(
            policy.observe(TaskStatus::Settled, &work_then_refusal()),
            TurnStep::GiveUp(TurnFault::Refused)
        );
        assert_eq!(TurnFault::Refused.given_up(), TaskStatus::Settled);
        assert_eq!(driven(TaskStatus::Settled, "failed").row_status(), "failed");
    }

    #[test]
    fn empty_and_refused_attempts_share_one_budget() {
        let mut policy = RepostPolicy::default();
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::Retry(TurnFault::Empty)
        );
        assert_eq!(
            policy.observe(TaskStatus::Settled, &work_then_refusal()),
            TurnStep::Retry(TurnFault::Refused)
        );
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::GiveUp(TurnFault::Empty)
        );
    }

    #[test]
    fn a_failure_that_is_not_a_refusal_is_never_retried() {
        let mut policy = RepostPolicy::default();
        assert_eq!(
            policy.observe(TaskStatus::Settled, &work_then_silence()),
            TurnStep::Keep(TaskStatus::Settled)
        );
        assert_eq!(policy.retries_used(), 0);
    }

    #[test]
    fn a_refusal_and_its_repost_are_counted_apart_from_empty_completions() {
        let mut task = driven(TaskStatus::Idle, "idle");
        task.record_fault(TurnFault::Refused);
        task.record_repost(TurnFault::Refused);
        task.record_fault(TurnFault::Empty);
        assert_eq!((task.refusals, task.refusal_retries), (1, 1));
        assert_eq!((task.empty_completions, task.empty_retries), (1, 0));
    }

    #[test]
    fn a_turn_with_no_text_and_no_tool_call_produced_nothing() {
        assert!(nothing().produced_nothing());
    }

    #[test]
    fn a_terse_answer_is_never_read_as_an_empty_completion() {
        assert!(!terse_answer().produced_nothing());
    }

    #[test]
    fn a_turn_of_tool_calls_closing_in_silence_is_never_read_as_empty() {
        assert!(!work_then_silence().produced_nothing());
    }

    #[test]
    fn a_billed_round_that_returned_nothing_is_still_an_empty_completion() {
        let billed = TurnOutput {
            input_total: 34_111,
            ..nothing()
        };
        assert!(billed.produced_nothing());
    }

    #[test]
    fn an_empty_first_turn_that_answers_on_the_retry_is_kept() {
        let mut policy = RepostPolicy::default();
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::Retry(TurnFault::Empty)
        );
        assert_eq!(
            policy.observe(TaskStatus::Idle, &terse_answer()),
            TurnStep::Keep(TaskStatus::Idle)
        );
        assert_eq!(policy.retries_used(), 1);
    }

    #[test]
    fn a_turn_that_never_answers_gives_up_after_two_retries() {
        let mut policy = RepostPolicy::default();
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::Retry(TurnFault::Empty)
        );
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::Retry(TurnFault::Empty)
        );
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::GiveUp(TurnFault::Empty)
        );
        assert_eq!(policy.retries_used(), REPOSTS_PER_TURN);
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::GiveUp(TurnFault::Empty)
        );
    }

    #[test]
    fn only_a_finished_or_refused_turn_is_ever_retried() {
        let mut policy = RepostPolicy::default();
        assert_eq!(
            policy.observe(TaskStatus::IdleAfterWake, &nothing()),
            TurnStep::Retry(TurnFault::Empty)
        );
        for status in [TaskStatus::Timeout, TaskStatus::Parked, TaskStatus::Settled] {
            let mut policy = RepostPolicy::default();
            assert_eq!(
                policy.observe(status, &nothing()),
                TurnStep::Keep(status),
                "{status:?}"
            );
            assert_eq!(policy.retries_used(), 0);
        }
        for status in [TaskStatus::Timeout, TaskStatus::Parked] {
            let mut policy = RepostPolicy::default();
            assert_eq!(
                policy.observe(status, &work_then_refusal()),
                TurnStep::Keep(status),
                "{status:?}"
            );
        }
    }

    #[test]
    fn an_unrecovered_empty_completion_is_written_under_its_own_name() {
        assert_eq!(
            driven(TaskStatus::Empty, "idle").row_status(),
            "empty-completion"
        );
    }

    #[test]
    fn an_empty_turn_two_beats_a_clean_turn_one() {
        assert_eq!(
            worse_of(TaskStatus::Idle, TaskStatus::Empty),
            TaskStatus::Empty
        );
        assert_eq!(
            worse_of(TaskStatus::IdleAfterWake, TaskStatus::Empty),
            TaskStatus::Empty
        );
        assert!(!worse_of(TaskStatus::Idle, TaskStatus::Empty).completed());
        assert_eq!(
            followup_to_send(&task(Some("more")), TaskStatus::Empty, "eval-7f", "rule"),
            None
        );
    }

    fn settle(readings: &[i64]) -> SettleStep {
        let mut settling = Settling::default();
        let mut step = SettleStep::Poll;
        for reading in readings {
            step = settling.observe(*reading, false);
        }
        step
    }

    #[test]
    fn one_reading_is_never_enough_to_snapshot_on() {
        assert_eq!(settle(&[40]), SettleStep::Poll);
    }

    #[test]
    fn a_still_log_settles_after_the_whole_quiet_run() {
        let short = vec![40i64; QUIET_OBSERVATIONS_FOR_SETTLED as usize];
        assert_eq!(settle(&short), SettleStep::Poll, "one reading short");
        let whole = vec![40i64; QUIET_OBSERVATIONS_FOR_SETTLED as usize + 1];
        assert_eq!(settle(&whole), SettleStep::Ready);
    }

    #[test]
    fn a_title_landing_late_restarts_the_quiet_run() {
        let mut readings = vec![40i64; QUIET_OBSERVATIONS_FOR_SETTLED as usize];
        assert_eq!(settle(&readings), SettleStep::Poll, "one reading short");

        readings.extend([41i64, 42]);
        assert_eq!(settle(&readings), SettleStep::Poll, "it just landed");

        readings.extend(vec![42i64; QUIET_OBSERVATIONS_FOR_SETTLED as usize - 1]);
        assert_eq!(settle(&readings), SettleStep::Poll, "one reading short");
        readings.push(42);
        assert_eq!(settle(&readings), SettleStep::Ready);
    }

    #[test]
    fn a_log_still_growing_at_the_bound_gives_up_rather_than_waiting() {
        let mut settling = Settling::default();
        assert_eq!(settling.observe(40, false), SettleStep::Poll);
        assert_eq!(settling.observe(41, true), SettleStep::GaveUp);
    }

    #[test]
    fn a_log_still_on_the_deadline_reading_is_ready_and_never_a_give_up() {
        let mut settling = Settling::default();
        for _ in 0..QUIET_OBSERVATIONS_FOR_SETTLED {
            assert_eq!(settling.observe(40, false), SettleStep::Poll);
        }
        assert_eq!(settling.observe(40, true), SettleStep::Ready);
    }

    #[test]
    fn the_same_readings_settle_the_same_way_whichever_arm_made_them() {
        let mut control = Settling::default();
        let mut lean = Settling::default();
        for reading in [40i64, 40, 41, 41, 42] {
            assert_eq!(
                control.observe(reading, false),
                lean.observe(reading, false)
            );
        }
        assert_eq!(control, lean);
    }

    #[test]
    fn the_retry_rule_is_the_same_whichever_arm_hit_the_empty_completion() {
        let mut control = RepostPolicy::default();
        let mut lean = RepostPolicy::default();
        for reading in [nothing(), nothing(), nothing()] {
            assert_eq!(
                control.observe(TaskStatus::Idle, &reading),
                lean.observe(TaskStatus::Idle, &reading)
            );
        }
        assert_eq!(control, lean);
    }
}
