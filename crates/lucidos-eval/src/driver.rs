//! Send a task's prompt, find its thread, poll it to the end, and answer
//! whatever it asks along the way.
//!
//! Nothing here scores anything. Its whole contract is one thread id and one
//! terminal status per task. The scoring layer then reads a finished thread
//! rather than racing one.
//!
//! **Idle is not the end.** A thread parked on an `await_event` subscription
//! reads `idle` in the projection, because ADR 0049 retired
//! `waiting_for_event`. Its work resumes when the event lands, so leaving at
//! the first idle records a partial task as a clean one. The park signal is
//! `thread_summaries.live_event_wait_count`, and [`DriveProgress`] is the rule
//! that reads it.
//!
//! **A task may take two turns.** One carrying a `followup` gets a second user
//! message in the same thread, once turn one is genuinely over. That is how a
//! task reaches the cross-turn trim, which drops the oldest history and says
//! nothing. Both turns share one deadline and one answer script.
//!
//! **A turn that produced nothing is re-posted, never scored.** The model can
//! end a turn with no text and no tool call. The engine is right to treat a
//! clean stop as benign silence, and a task turn is not silence: the thread
//! never started the work. [`EmptyCompletionPolicy`] re-posts the same prompt,
//! and gives up after [`EMPTY_COMPLETION_RETRIES`]. The caller then voids the
//! task in both arms, because a thread that never ran did not fail to deliver.
//!
//! **A finished turn is not the end of its spend.** The title, the memory
//! extractor and the summariser run in detached tasks, and each bills real
//! tokens. [`wait_until_settled`] holds the snapshot until the thread's event
//! log stops moving, and records the fact when it never does.
//!
//! Threads are never archived between tasks. The accumulated state is the
//! independent variable.

use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::config::{Task, TaskAnswers};
use crate::results::{EMPTY_STATUS, IDLE_STATUS, PARKED_STATUS, WOKEN_STATUS};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How often the driver asks the projection what a thread is doing.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How long a thread may take to appear in `thread_summaries` after its prompt
/// is accepted. Separate from the task timeout: a thread that never appears is
/// a harness failure, and a thread that never finishes is a result.
const RESOLVE_TIMEOUT: Duration = Duration::from_secs(60);

/// What the driver replies once a task's scripted answers run out.
///
/// **It closes scope rather than opening it.** A question the script did not
/// anticipate is drift by definition. So the reply sends the agent back to the
/// request, instead of authorising whatever it thought of. This used to read
/// "use your judgment", which is what green-lit T06's invented second job: 18
/// rounds against the other arm's 5, on a question only one arm was asked.
///
/// Still recorded per thread, because an unscripted answer means the run
/// drifted from the script and the reader has to know which threads that
/// touched.
pub const UNSCRIPTED_REPLY: &str =
    "Stick to the original request and leave the rest. Mention anything further at the end of \
     your reply.";

/// Device this harness registers so its messages count as human.
///
/// The engine refuses a `mode: "human"` claim it has no evidence for. The eval
/// is a real external client, so it registers rather than weakening the gate.
pub const EVAL_DEVICE_ID: &str = "lucidos-eval-driver";

/// How a driven task ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// The thread reached idle inside its timeout, holding no event wait.
    Idle,
    /// The thread parked on an event wait, woke, and then reached idle. A
    /// complete task, and kept apart from the plain finish because the rounds
    /// after the wake are the ones the old driver threw away.
    IdleAfterWake,
    /// Every attempt at a turn came back with no text and no tool call, so the
    /// thread never started the task. Voided rather than scored.
    Empty,
    /// The deadline arrived while the thread still held an event wait. The
    /// event never landed, so nothing will resume it.
    Parked,
    /// The task timeout fired first. Downstream probes are voided.
    Timeout,
    /// The thread settled somewhere that is not idle, for example `failed`.
    Settled,
}

impl TaskStatus {
    /// Whether the thread reached an end of its own, a wake included.
    ///
    /// An empty completion reached idle and is still not an end. The thread
    /// produced nothing, so there is no work for a follow-up to build on, and
    /// nothing for a probe to read.
    pub fn completed(self) -> bool {
        matches!(self, TaskStatus::Idle | TaskStatus::IdleAfterWake)
    }

    /// How bad this outcome is, so two turns can be reduced to one status.
    ///
    /// A wake ranks above a plain finish because it is the louder fact about a
    /// thread, and both are finishes. An empty completion ranks above both and
    /// below the rest, which only orders it against a finished turn one. An
    /// empty turn one never earns a turn two.
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

/// The status a two-turn task records: the worse of its turns.
///
/// The row describes the whole task rather than its last turn. A first turn
/// that timed out never gets a follow-up, so it never reaches here. What this
/// catches is the other order: a clean turn one and a turn two that timed out
/// must not read as a clean finish.
pub fn worse_of(first: TaskStatus, second: TaskStatus) -> TaskStatus {
    match second.severity() > first.severity() {
        true => second,
        false => first,
    }
}

/// What one driven task produced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrivenTask {
    pub task: String,
    pub thread_id: Uuid,
    pub status: TaskStatus,
    pub terminal_status: String,
    pub unscripted_answers: u32,
    pub questions_answered: u32,
    /// Attempts at a turn that came back with nothing. Every attempt counts, so
    /// a turn re-posted twice and still empty contributes three.
    pub empty_completions: u32,
    /// Prompts re-posted to recover one of those turns.
    pub empty_retries: u32,
    /// Sequence of the prompt that opened turn two, once one was driven.
    ///
    /// Recorded because a re-post writes its own `MessageReceived`. Counting
    /// prompts then finds a turn-one attempt where the boundary should be.
    pub followup_sequence: Option<i64>,
}

impl DrivenTask {
    /// Whether the task ran to the end, a wake included.
    pub fn completed(&self) -> bool {
        self.status.completed()
    }

    /// What the results row records as this task's status.
    ///
    /// A park and a wake need their own words. Both read `idle` in the
    /// projection, so recording that would hide the abandonment this
    /// distinction exists to surface.
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

/// One poll of a thread's projection row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadState {
    pub status: String,
    /// Event waits the thread itself holds unresolved. Above zero, something
    /// will wake it.
    pub live_event_waits: i64,
}

/// What one observation tells the driver to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Nothing to do but poll again.
    Poll,
    /// A question is pending. Answer it, then poll again.
    Answer,
    /// The task is over, with this status.
    Stop(TaskStatus),
}

/// Quiet polls a woken thread needs before the driver calls it finished.
///
/// A wait resolves a few milliseconds before the thread starts its next round.
/// In the run that motivated this, `EventWaitDelivered` and
/// `UserPromptInjected` were six milliseconds apart. One poll landing in that
/// gap sees an idle thread with no wait and would leave, which is the bug
/// again, one round later. Two polls put four seconds against six
/// milliseconds.
const QUIET_POLLS_AFTER_WAKE: u32 = 2;

/// What the driver carries between polls of one thread.
///
/// The two flags below used to be one. `parked` named the finish and set the
/// quiet run at the same time. That left no way to demand a quiet run for a
/// reason other than a wake, and a posted follow-up is exactly that reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveProgress {
    /// The thread has held an event wait at some point.
    parked: bool,
    /// Consecutive polls seeing an idle thread with no wait.
    quiet_polls: u32,
    /// How many of those it takes before the driver calls this turn finished.
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
    /// Start a turn the driver has just posted a prompt into.
    ///
    /// The thread reads idle until the engine picks the message up, which is
    /// the same race a wake runs. One poll landing in that gap would record an
    /// untouched turn two as a finished one.
    pub fn after_prompt() -> Self {
        DriveProgress {
            quiet_polls_needed: QUIET_POLLS_AFTER_WAKE,
            ..DriveProgress::default()
        }
    }

    /// Fold one observation in, and say what to do about it.
    ///
    /// Reads no arm. Both arms are driven by exactly this rule, or the wake
    /// lands in one arm's measurement and not the other's.
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
            // A thread that reached its own end beats the clock. The deadline
            // decides an unfinished task, not a finished one.
            stop @ Step::Stop(_) => stop,
            _ if past_deadline => Step::Stop(self.at_deadline(state)),
            step => step,
        }
    }

    fn classify(&self, state: &ThreadState) -> Step {
        match state.status.as_str() {
            // Neither settles by waiting, so a held wait changes nothing here.
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

    /// A turn with nothing to race stops on the first quiet poll, as before.
    fn quiet_enough(&self) -> bool {
        self.quiet_polls >= self.quiet_polls_needed
    }

    /// A wait still open at the deadline is a park, and never a plain timeout.
    fn at_deadline(&self, state: &ThreadState) -> TaskStatus {
        match state.live_event_waits > 0 {
            true => TaskStatus::Parked,
            false => TaskStatus::Timeout,
        }
    }
}

/// How many times a turn that produced nothing is re-posted before the driver
/// gives up on it.
///
/// Two. Every attempt is billed, and the failure this covers is a provider
/// hiccup rather than a state the model argues itself out of. The budget is per
/// turn, so a two-turn task can spend it twice.
pub const EMPTY_COMPLETION_RETRIES: u32 = 2;

/// What one turn of a thread put into the event store.
///
/// Read once the turn reached a terminal state, so nothing is still being
/// written. Every field is scoped to the turn by sequence: an earlier turn's
/// work says nothing about this one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TurnOutput {
    /// Whether any `ResponseGenerated` in the turn carried text.
    pub said_something: bool,
    /// `ToolCalled` events the turn wrote.
    pub tool_calls: i64,
    /// Tokens the turn's rounds processed on input, cached ones included.
    ///
    /// A size, never a bill. Deriving what was billed means taking the cached
    /// counts out first, which is `InputSplit`'s job and not this probe's.
    pub input_total: i64,
}

impl TurnOutput {
    /// Whether the model ended the turn without doing anything at all.
    ///
    /// Both halves are needed. Empty text alone fires on a turn whose work was
    /// tool calls and whose closing message was deliberately empty, which is a
    /// real attempt. No tool calls alone fires on a terse answer, which is one
    /// too.
    ///
    /// `input_total` is deliberately not part of it. A zero round trip
    /// corroborates the diagnosis and is logged. A provider that billed the
    /// round and returned nothing produced the same non-attempt.
    pub fn produced_nothing(&self) -> bool {
        !self.said_something && self.tool_calls == 0
    }
}

/// What to do with a turn that has just reached a terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStep {
    /// The turn produced something, or ended in a way a re-post cannot fix.
    Keep(TaskStatus),
    /// The turn produced nothing. Re-post its prompt and drive it again.
    Retry,
    /// It produced nothing every time. The task is voided, not scored.
    GiveUp,
}

/// The retry budget of one turn, and the rule that spends it.
///
/// Its own state machine for the same reason [`DriveProgress`] is one: the
/// decision is worth testing without a database, and both arms have to run
/// exactly this rule (ADR 0087 I7). Nothing here reads an arm.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EmptyCompletionPolicy {
    retries_used: u32,
}

impl EmptyCompletionPolicy {
    /// Fold in one finished attempt, and say what to do about it.
    pub fn observe(&mut self, status: TaskStatus, output: &TurnOutput) -> TurnStep {
        // A turn that timed out, parked or settled produced no result either,
        // and a re-post would measure the same wall clock again. Those keep
        // their own status, which the caller already voids downstream.
        if !status.completed() || !output.produced_nothing() {
            return TurnStep::Keep(status);
        }
        if self.retries_used >= EMPTY_COMPLETION_RETRIES {
            return TurnStep::GiveUp;
        }
        self.retries_used += 1;
        TurnStep::Retry
    }

    /// Re-posts spent so far, which the thread row records.
    pub fn retries_used(&self) -> u32 {
        self.retries_used
    }
}

/// Observations of a still event log before a finished thread is snapshotted.
///
/// Stillness cannot prove a detached task finished, because a call in flight
/// writes nothing. What it proves is elapsed time. Title generation starts when
/// the follow-up prompt is posted, which is before every event the turn then
/// wrote. So a log still for this many observations has been still for that
/// long since the detached task began.
///
/// Ten, at the interval below, is ten seconds. That covers a resampled title,
/// which is two calls to the auxiliary model.
const QUIET_OBSERVATIONS_FOR_SETTLED: u32 = 10;

/// How often the settling wait re-reads the thread's highest sequence.
const SETTLE_INTERVAL: Duration = Duration::from_secs(1);

/// How long the settling wait may run before it snapshots anyway.
///
/// Only a log that keeps growing reaches this. A still one settles long before,
/// so the bound is generous.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(120);

/// What one reading of a finished thread's event log says to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettleStep {
    /// The log moved, or has not been still for the whole run. Read again.
    Poll,
    /// It has been still for the whole run. Snapshot now.
    Ready,
    /// The bound arrived with the log still growing. Snapshot, and say so.
    GaveUp,
}

/// The rule that decides when a finished thread is safe to snapshot.
///
/// Its own state machine for the same reason [`DriveProgress`] is one: the
/// decision is worth testing without a database, and both arms run exactly this
/// rule (ADR 0087 I7). Nothing here reads an arm.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settling {
    /// The highest sequence seen so far, unset before the first reading.
    highest: Option<i64>,
    /// Consecutive readings that found the log where they left it.
    quiet: u32,
}

impl Settling {
    /// Fold one reading of the thread's highest sequence in.
    ///
    /// A reading that moved the mark restarts the run. A detached title landing
    /// late is one such move, and it is the whole reason for the wait.
    pub fn observe(&mut self, highest: i64, past_deadline: bool) -> SettleStep {
        match self.highest {
            Some(mark) if mark == highest => self.quiet += 1,
            _ => {
                self.highest = Some(highest);
                self.quiet = 0;
            }
        }
        match (self.quiet >= QUIET_OBSERVATIONS_FOR_SETTLED, past_deadline) {
            // A log that went still beats the clock, exactly as a finished
            // thread beats it in `DriveProgress`.
            (true, _) => SettleStep::Ready,
            (false, true) => SettleStep::GaveUp,
            (false, false) => SettleStep::Poll,
        }
    }
}

/// Wait for a finished thread to stop writing, and say whether it did.
///
/// The title, the memory extractor and the summariser run in detached tasks the
/// harness cannot join. A snapshot taken the moment the driver leaves records
/// whichever of them had landed. Two repeats of one task then differ by
/// scheduling rather than by the arm.
///
/// `true` means the log went still for the whole quiet run. It is not a
/// completion, because a call in flight writes nothing that a poll can see. The
/// caller stores it as `event_log_settled` so the row claims only that.
///
/// `false` means the bound arrived with the log still growing. The caller
/// records that on the thread row. A snapshot taken over a moving target is
/// then visible in the results, rather than skewing a total in silence.
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

/// One arm's live endpoints, as the driver sees them.
pub struct ArmEndpoint {
    pub base_url: String,
    pub client: reqwest::Client,
    pub pool: PgPool,
}

impl ArmEndpoint {
    /// Build a client that speaks for a registered device.
    pub async fn connect(base_url: &str, pool: PgPool) -> Fallible<ArmEndpoint> {
        let bootstrap = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?;
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
            client: reqwest::Client::builder()
                .danger_accept_invalid_certs(true)
                .default_headers(headers)
                .build()?,
            pool,
        })
    }
}

/// Send one task and drive it to a terminal state.
///
/// A task carrying a `followup` runs twice in the same thread. One deadline
/// covers both turns, and the scripted answers are one list consumed in order
/// across them. The recorded status describes the whole task, so a turn two
/// that timed out is a timeout however cleanly turn one finished.
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
    // Turn two's own opening prompt, and never turn one's. A re-posted turn one
    // leaves several prompts behind it, so the boundary is recorded rather than
    // counted.
    driven.followup_sequence = Some(second.opened_at);
    driven.status = worse_of(driven.status, second.status);
    Ok(driven)
}

/// What one task's turns share, and what they consume across both.
struct Turns<'a> {
    /// Prompts the driver has posted on this thread, re-posts included. It
    /// resolves the sequence the current turn started at.
    prompts_posted: usize,
    scripted_used: usize,
    deadline: Instant,
    answers: Option<&'a TaskAnswers>,
}

/// How one turn ended, and which prompt opened the attempt that ended that way.
struct DrivenTurn {
    status: TaskStatus,
    opened_at: i64,
}

/// Drive one turn, re-posting its prompt while it comes back with nothing.
///
/// The prompt is posted by the caller, so the first attempt only drives. Each
/// re-post opens a fresh turn on the same thread, and the emptiness question is
/// asked of that turn alone.
async fn drive_one_turn(
    arm: &ArmEndpoint,
    driven: &mut DrivenTask,
    prompt: &str,
    mut progress: DriveProgress,
    turns: &mut Turns<'_>,
) -> Fallible<DrivenTurn> {
    let mut policy = EmptyCompletionPolicy::default();
    loop {
        let opened_at = await_prompt(
            &arm.pool,
            driven.thread_id,
            turns.prompts_posted,
            &driven.task,
        )
        .await?;
        let status = drive_turn(arm, driven, progress, turns).await?;
        let output = turn_output(&arm.pool, driven.thread_id, opened_at).await?;
        let step = policy.observe(status, &output);
        if let TurnStep::Keep(status) = step {
            return Ok(DrivenTurn { status, opened_at });
        }
        driven.empty_completions += 1;
        report_empty_completion(&driven.task, &output, step, policy.retries_used());
        if step == TurnStep::GiveUp {
            return Ok(DrivenTurn {
                status: TaskStatus::Empty,
                opened_at,
            });
        }
        post_message(arm, prompt, Some(driven.thread_id)).await?;
        turns.prompts_posted += 1;
        // Accumulated, because the budget is per turn and the count is per
        // task. Assigning the policy's total would let turn two's first re-post
        // erase turn one's.
        driven.empty_retries += 1;
        progress = DriveProgress::after_prompt();
    }
}

/// Say that a turn produced nothing, and what is being done about it.
///
/// The input tokens go on the line because they are the corroborating tell.
/// Zero means the provider never ran the request. A billed round that returned
/// nothing is a different fault wearing the same symptom.
fn report_empty_completion(task: &str, output: &TurnOutput, step: TurnStep, attempt: u32) {
    let next = match step {
        TurnStep::GiveUp => "giving up, and the task is voided in both arms".to_string(),
        _ => format!("re-posting the prompt, attempt {attempt} of {EMPTY_COMPLETION_RETRIES}"),
    };
    println!(
        "[eval]   {task} ended a turn with no text and no tool call ({} input tokens): {next}",
        output.input_total
    );
}

/// The follow-up to post, if the task carries one and turn one earned it.
///
/// A turn that timed out, parked or settled did not finish. Posting into it
/// would blur the two turns into one and would record a partial first turn as
/// the setup for a clean second. The task then measures nothing, which reads
/// worse than the failure it hides.
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

/// The request body for one prompt.
///
/// A follow-up MUST name its thread. `/api/v1/chat/stream` reads a missing
/// `thread_id` as "mint one", so an unaddressed second prompt opens a fresh
/// thread. Turn two would then land where no probe reads it. Turn one names
/// nothing, because there is nothing yet to name.
fn chat_body(message: &str, thread: Option<Uuid>) -> serde_json::Value {
    serde_json::json!({
        "message": message,
        "mode": "human",
        "thread_id": thread.map(|id| id.to_string()),
    })
}

/// Poll one turn to its end, answering whatever it asks along the way.
///
/// Everything it accumulates lands on `driven`, so the counts cover the task
/// rather than the turn. The status comes back separately, because only the
/// caller knows whether another turn follows.
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

/// Wait for the driver's nth prompt to land, and say which sequence it took.
///
/// The boundary comes from the thread rather than from the clock. Driving on a
/// timer would let a slow write land mid-turn, and every probe reading "after
/// the boundary" would then read a boundary that moved.
///
/// The count is the driver's own, and it includes a re-post. Waiting for "more
/// than one prompt" would return at once on a re-posted turn one. Turn two
/// would then be driven before its prompt had landed.
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

/// What one turn put into the event store, read after the turn ended.
///
/// Scoped by the sequence of the prompt that opened the turn, so an earlier
/// turn's work never answers for this one. Text is counted over every
/// `ResponseGenerated` in the turn rather than the last: a turn that said
/// something and then closed empty still said something.
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
                        AND event_type = 'ContextCaptured'), 0)::bigint AS input_total",
    )
    .bind(thread_id)
    .bind(opened_at)
    .fetch_one(pool)
    .await?;
    Ok(TurnOutput {
        said_something: row.try_get("said_something")?,
        tool_calls: row.try_get("tool_calls")?,
        input_total: row.try_get("input_total")?,
    })
}

/// Find the thread this task's prompt created, by its unique marker.
///
/// The marker beats "the newest thread": repeats run in parallel against
/// separate databases, but a task can also spawn a sub-thread, and the newest
/// row is then the child.
pub async fn resolve_thread_by_marker(pool: &PgPool, marker: &str) -> Fallible<Uuid> {
    let pattern = format!("%{marker}%");
    let deadline = Instant::now() + RESOLVE_TIMEOUT;
    loop {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT thread_id FROM thread_summaries WHERE first_message LIKE $1 LIMIT 1",
        )
        .bind(&pattern)
        .fetch_optional(pool)
        .await?;
        if let Some((thread_id,)) = row {
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

/// What the thread is doing, and whether anything will wake it.
///
/// Both columns in one read, so the pair cannot come from two moments. Read
/// apart, a wait resolving between the two queries reads as an idle thread that
/// never parked.
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

/// A question waiting for an answer.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingQuestion {
    pub tool_use_id: String,
    pub text: String,
    /// Offered options as (id, label) pairs, empty for a free-text question.
    pub options: Vec<(String, String)>,
}

/// The newest `UserQuestionAsked` on the thread with no answer yet.
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

/// What the driver decided to reply.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: String,
    pub unscripted: bool,
}

/// Pick the scripted answer for a question, or the unscripted fallback.
///
/// Scripted answers are consumed in order, each used once. A task asking the
/// same question twice gets the script's second entry, then the fallback. That
/// is what makes `unscripted_answers` mean anything.
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

/// The answer body for one reply, resolved against the offered options.
///
/// A `Selected` answer names an option id, so a reply that matches an offered
/// label is sent as that option. Anything else goes as free text, which is also
/// the only shape a question with no options can take.
///
/// **An unscripted reply is always free text**, whatever the labels say. It is
/// a refusal to pick rather than a pick, so choosing an option would be the
/// wrong answer. Matching is also substring-based both ways, so a one-word
/// label can hide inside any sentence: "No" sits inside "note". Keying this on
/// the flag makes the outcome a property of the reply rather than of the
/// wording.
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

/// The thread's final response text, which several probes read.
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

/// A per-run token that resolves one task's thread.
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

    /// The fallback closes scope. "use your judgment" is what authorised
    /// T06's invented second job, and the arm paid for it.
    #[test]
    fn the_unscripted_reply_sends_the_agent_back_to_the_request() {
        let lowered = UNSCRIPTED_REPLY.to_ascii_lowercase();
        assert!(!lowered.contains("judgment") && !lowered.contains("judgement"));
        assert!(lowered.contains("original request"));
    }

    /// Substring matching runs both ways, so a one-word label hides inside any
    /// sentence. "No" sits inside "Mention". An unscripted reply is a refusal
    /// to pick, so it must never arrive as a pick.
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

    /// The guard is keyed on the flag, not the wording, so it holds for any
    /// text the fallback ever carries.
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
        // The same text from the script still selects the option.
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
        let control = task_marker("abc", 1, crate::arm::Arm::Control, "T05");
        let lean = task_marker("abc", 1, crate::arm::Arm::Lean, "T05");
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
            followup_sequence: None,
        }
    }

    /// A wake is a finish. A park, a timeout and an empty completion are not.
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

    /// The projection calls a park and a finish `idle` alike, so the row may
    /// not repeat it. A parked row reading `idle` is the whole defect.
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

    /// The defect, as one observation. Something will wake this thread, so the
    /// driver has no business leaving.
    #[test]
    fn an_idle_thread_holding_an_event_wait_is_not_finished() {
        let mut progress = DriveProgress::default();
        assert_eq!(progress.observe(&state("idle", 1), false), Step::Poll);
    }

    /// Unchanged for every task that never parks, which is most of them.
    #[test]
    fn a_thread_that_never_parked_finishes_on_the_first_quiet_poll() {
        let mut progress = DriveProgress::default();
        assert_eq!(
            progress.observe(&state("idle", 0), false),
            Step::Stop(TaskStatus::Idle)
        );
    }

    /// T08's shape: park, wake, work, finish. The rounds after the wake are
    /// what the old driver threw away.
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

    /// The delivery race. A single quiet poll can land between the wait
    /// resolving and the thread starting its next round, so the quiet run has
    /// to be consecutive.
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

    /// A park that never resolves ends the run visibly. Recorded as a normal
    /// idle finish it would look like a task that simply had little to do.
    #[test]
    fn a_wait_still_open_at_the_deadline_is_a_park_and_never_a_timeout() {
        let mut progress = DriveProgress::default();
        assert_eq!(
            progress.observe(&state("idle", 1), true),
            Step::Stop(TaskStatus::Parked)
        );
    }

    /// The wake landed and the thread is simply slow. That is an ordinary
    /// timeout, and calling it a park would blame the subscription.
    #[test]
    fn a_deadline_reached_after_the_wake_is_an_ordinary_timeout() {
        let mut progress = DriveProgress::default();
        assert_eq!(progress.observe(&state("idle", 1), false), Step::Poll);
        assert_eq!(
            progress.observe(&state("running", 0), true),
            Step::Stop(TaskStatus::Timeout)
        );
    }

    /// A task that got there beats the clock. The deadline decides an
    /// unfinished task, never a finished one.
    #[test]
    fn a_thread_that_finished_on_the_deadline_poll_is_not_a_timeout() {
        let mut progress = DriveProgress::default();
        assert_eq!(
            progress.observe(&state("idle", 0), true),
            Step::Stop(TaskStatus::Idle)
        );
    }

    /// Neither status settles by waiting, so a held wait changes nothing.
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

    /// A question blocks the thread whatever else it holds, and answering is
    /// what lets the wait matter later.
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

    /// Most tasks carry no follow-up, and nothing about them changed.
    #[test]
    fn a_task_with_no_followup_sends_one_prompt_and_stops() {
        assert_eq!(
            followup_to_send(&task(None), TaskStatus::Idle, "eval-7f", "stay in scope"),
            None
        );
    }

    /// The prompt is rendered exactly as turn one's is, marker and rule
    /// included, so the ceiling cannot take the standing rule off one arm.
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
        // A wake is a finish too, so it earns the follow-up as well.
        assert!(followup_to_send(
            &task(Some("more")),
            TaskStatus::IdleAfterWake,
            "eval-7f",
            "stay in scope"
        )
        .is_some());
    }

    /// A turn one that never finished is not the setup for turn two. Posting
    /// into it would blur the boundary the task exists to measure.
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

    /// An unaddressed follow-up opens a second thread, and every probe then
    /// reads the first one. The turn-one body names no thread on purpose.
    #[test]
    fn a_followup_names_the_thread_it_belongs_to() {
        let thread = Uuid::from_u128(0x5eed);
        let second = chat_body("now tell me the hold time", Some(thread));
        assert_eq!(second["thread_id"], thread.to_string());

        let first = chat_body("read the five documents", None);
        assert!(first["thread_id"].is_null());
        assert_eq!(first["mode"], "human");
    }

    /// A posted prompt races the projection as a wake does. So turn two needs
    /// the same consecutive quiet run before the driver calls it done.
    #[test]
    fn a_turn_opened_by_a_prompt_needs_the_quiet_run_a_wake_needs() {
        let mut progress = DriveProgress::after_prompt();
        assert_eq!(progress.observe(&state("idle", 0), false), Step::Poll);
        assert_eq!(
            progress.observe(&state("idle", 0), false),
            Step::Stop(TaskStatus::Idle)
        );
    }

    /// Turn two never parked, so its own finish is a plain one. The wake
    /// belongs to turn one, and the combined status is where it survives.
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

    /// The row describes the task, not its last turn. A turn two that timed
    /// out must not read as the clean finish turn one had.
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

    /// Both arms run this rule and nothing in it reads an arm (I7).
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

    /// T06 lean of run 6adb6572: one round, no tokens, no work, no answer.
    fn nothing() -> TurnOutput {
        TurnOutput {
            said_something: false,
            tool_calls: 0,
            input_total: 0,
        }
    }

    /// Control T11 of the same run: one round, an answer, no tool call.
    fn terse_answer() -> TurnOutput {
        TurnOutput {
            said_something: true,
            tool_calls: 0,
            input_total: 35_628,
        }
    }

    /// T05 lean and T06 control of the same run: four rounds of tool work, and
    /// a closing message the model left empty.
    fn work_then_silence() -> TurnOutput {
        TurnOutput {
            said_something: false,
            tool_calls: 9,
            input_total: 177_589,
        }
    }

    /// The defect, as one reading. Nothing was said and nothing was done.
    #[test]
    fn a_turn_with_no_text_and_no_tool_call_produced_nothing() {
        assert!(nothing().produced_nothing());
    }

    /// The false positive that would matter most. A short answer is an answer,
    /// and re-posting the prompt would charge for a turn that already worked.
    #[test]
    fn a_terse_answer_is_never_read_as_an_empty_completion() {
        assert!(!terse_answer().produced_nothing());
    }

    /// The other false positive. Two of the three empty completions in the
    /// motivating run were this shape, and both threads had done the work.
    #[test]
    fn a_turn_of_tool_calls_closing_in_silence_is_never_read_as_empty() {
        assert!(!work_then_silence().produced_nothing());
    }

    /// A billed round trip that returned nothing is the same non-attempt. The
    /// tokens corroborate the diagnosis and never make it.
    #[test]
    fn a_billed_round_that_returned_nothing_is_still_an_empty_completion() {
        let billed = TurnOutput {
            input_total: 34_111,
            ..nothing()
        };
        assert!(billed.produced_nothing());
    }

    /// The shape this whole change exists for: one empty completion, then a
    /// re-post that works. The task scores, and the count says it happened.
    #[test]
    fn an_empty_first_turn_that_answers_on_the_retry_is_kept() {
        let mut policy = EmptyCompletionPolicy::default();
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::Retry
        );
        assert_eq!(
            policy.observe(TaskStatus::Idle, &terse_answer()),
            TurnStep::Keep(TaskStatus::Idle)
        );
        assert_eq!(policy.retries_used(), 1);
    }

    /// Two re-posts, and no more. The third empty completion gives up, and the
    /// caller voids the task rather than recording a delivery failure.
    #[test]
    fn a_turn_that_never_answers_gives_up_after_two_retries() {
        let mut policy = EmptyCompletionPolicy::default();
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::Retry
        );
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::Retry
        );
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::GiveUp
        );
        assert_eq!(policy.retries_used(), EMPTY_COMPLETION_RETRIES);
        // And it stays given up, however many times it is asked.
        assert_eq!(
            policy.observe(TaskStatus::Idle, &nothing()),
            TurnStep::GiveUp
        );
    }

    /// A wake is a finish, so an empty woken turn is retried like any other.
    /// A turn that did not finish keeps its own status: re-posting into a
    /// timeout would just spend the deadline again.
    #[test]
    fn only_a_finished_turn_is_ever_retried() {
        let mut policy = EmptyCompletionPolicy::default();
        assert_eq!(
            policy.observe(TaskStatus::IdleAfterWake, &nothing()),
            TurnStep::Retry
        );
        for status in [TaskStatus::Timeout, TaskStatus::Parked, TaskStatus::Settled] {
            let mut policy = EmptyCompletionPolicy::default();
            assert_eq!(
                policy.observe(status, &nothing()),
                TurnStep::Keep(status),
                "{status:?}"
            );
            assert_eq!(policy.retries_used(), 0);
        }
    }

    /// An empty thread is written under its own name. Recorded as `idle` it
    /// would read as a task that ran and delivered nothing, which is the false
    /// signal that killed run 6adb6572's lean arm.
    #[test]
    fn an_unrecovered_empty_completion_is_written_under_its_own_name() {
        assert_eq!(
            driven(TaskStatus::Empty, "idle").row_status(),
            "empty-completion"
        );
    }

    /// A turn two that never ran must not read as turn one's clean finish, and
    /// an empty turn one never earns a turn two.
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

    /// Fold a run of sequence readings in, and hand back the last step.
    fn settle(readings: &[i64]) -> SettleStep {
        let mut settling = Settling::default();
        let mut step = SettleStep::Poll;
        for reading in readings {
            step = settling.observe(*reading, false);
        }
        step
    }

    /// The first reading only sets the mark. A thread is never called settled
    /// on the strength of one look at it.
    #[test]
    fn one_reading_is_never_enough_to_snapshot_on() {
        assert_eq!(settle(&[40]), SettleStep::Poll);
    }

    /// A log nothing touches settles once the run is complete, and not before.
    #[test]
    fn a_still_log_settles_after_the_whole_quiet_run() {
        let short = vec![40i64; QUIET_OBSERVATIONS_FOR_SETTLED as usize];
        assert_eq!(settle(&short), SettleStep::Poll, "one reading short");
        let whole = vec![40i64; QUIET_OBSERVATIONS_FOR_SETTLED as usize + 1];
        assert_eq!(settle(&whole), SettleStep::Ready);
    }

    /// T14's shape, and the defect this wait exists for. The follow-up starts a
    /// detached title call. Its capture lands after the driver has left, and
    /// the old harness had already snapshotted the thread without it.
    #[test]
    fn a_title_landing_late_restarts_the_quiet_run() {
        let mut readings = vec![40i64; QUIET_OBSERVATIONS_FOR_SETTLED as usize];
        assert_eq!(settle(&readings), SettleStep::Poll, "one reading short");

        // The title's capture and the title itself, two sequences apart.
        readings.extend([41i64, 42]);
        assert_eq!(settle(&readings), SettleStep::Poll, "it just landed");

        readings.extend(vec![42i64; QUIET_OBSERVATIONS_FOR_SETTLED as usize - 1]);
        assert_eq!(settle(&readings), SettleStep::Poll, "one reading short");
        readings.push(42);
        assert_eq!(settle(&readings), SettleStep::Ready);
    }

    /// A thread that never stops writing is snapshotted anyway, and the row
    /// records that it was. Waiting forever would hang the run.
    #[test]
    fn a_log_still_growing_at_the_bound_gives_up_rather_than_waiting() {
        let mut settling = Settling::default();
        assert_eq!(settling.observe(40, false), SettleStep::Poll);
        assert_eq!(settling.observe(41, true), SettleStep::GaveUp);
    }

    /// A log that went still beats the clock, exactly as a finished thread
    /// beats it in `DriveProgress`. The bound decides a moving log alone.
    #[test]
    fn a_log_still_on_the_deadline_reading_is_ready_and_never_a_give_up() {
        let mut settling = Settling::default();
        for _ in 0..QUIET_OBSERVATIONS_FOR_SETTLED {
            assert_eq!(settling.observe(40, false), SettleStep::Poll);
        }
        assert_eq!(settling.observe(40, true), SettleStep::Ready);
    }

    /// Both arms are snapshotted on the same rule (I7). Nothing in it reads an
    /// arm, and this is what keeps it that way.
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

    /// Both arms spend the same budget on the same rule (I7). Nothing in the
    /// policy reads an arm, and this is what keeps it that way.
    #[test]
    fn the_retry_rule_is_the_same_whichever_arm_hit_the_empty_completion() {
        let mut control = EmptyCompletionPolicy::default();
        let mut lean = EmptyCompletionPolicy::default();
        for reading in [nothing(), nothing(), nothing()] {
            assert_eq!(
                control.observe(TaskStatus::Idle, &reading),
                lean.observe(TaskStatus::Idle, &reading)
            );
        }
        assert_eq!(control, lean);
    }
}
