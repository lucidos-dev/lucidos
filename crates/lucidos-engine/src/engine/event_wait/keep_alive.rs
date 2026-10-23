//! The *cache keep-alive*: keep a waiting thread's prompt cache warm, for a
//! Claude Code session or the Lucidos Agent.
//!
//! The cache lives 5 minutes, 30 minutes or 1 hour from the start of the last
//! request. A longer wait makes the wake-up re-write the whole context. So
//! the engine beats just before the cache would expire, by one of three
//! routes:
//!
//! - **Resume.** An idle session waiting on a *keep-warm* event wait or a
//!   working child gets a hidden nudge. It answers in a word and idles again.
//! - **Session copy.** A live session parked on a question card or permission
//!   prompt blocks mid-turn, so a copy of it reads the same cached prefix. The
//!   session, its transcript and its card never hear of it.
//! - **Prefill.** A Lucidos Agent thread's last request goes to its provider
//!   again with no output. It is cut at the stable prefix for an event wait,
//!   and whole for the thread's own question card.
//!
//! Rules both routes keep:
//!
//! - **The TTL is observed, never guessed.** Claude Code's usage block splits
//!   each cache write by lifetime. An unknown TTL beats as 5 minutes.
//! - **Timing comes from persisted events**, so a restart causes no burst. A
//!   copy beat's capture moves the anchor as a turn request does.
//! - **A resume never contends for the thread.** It skips a live, held or busy
//!   thread, and one a person owes an answer.
//! - **A child parked on the user keeps nobody warm.** Its parent wakes only
//!   when a child finishes, long after the answer.
//! - **A `no-wake` child keeps nobody warm.** Its clean finish never wakes the
//!   parent, so nothing would read the warm cache.
//! - **The beats are invisible; the status is not.** The tick writes one *cache
//!   keep-alive status* per waiting thread, only on a change (`sync_status`).
//!   A parked thread shows none.
//! - **Every run gives up.** `cache_keep_alive_mode` picks the rule: break-even
//!   by default, half of it for a parked wait, or this workspace's cheapest cap.
//! - **The user can turn it off for one thread.** The switch holds until the
//!   thread's next real request, and acts on that thread alone.
//!
//! Why a keep-alive and not the 1-hour TTL: ADR 0470. Why the window follows
//! the observed TTL: ADR 0479. Why the budget follows the model's prices:
//! ADR 0489. Why it shows one status: ADR 0529. Why a copy beat: ADR 0547.
//! Why a per-thread switch: ADR 0554. Why prefill beats: ADR 0556.

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::remembered_requests::Remembered;
use super::wait_durations::WaitDurations;
use super::LiveWait;
use crate::engine::cache_ttl::CacheTtl;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::successor::{
    last_cache_refresh_started_sql, last_real_request_started_sql, last_turn_started_sql,
};
use crate::engine::thread_events::{
    ActorMode, CacheKeepAliveNotKeptReason, CacheKeepAliveStatus, EngineReason, EventChannel,
    EventMeta, MessageOrigin, ThreadEvent, WakeMode, CACHE_KEEP_ALIVE_REASON_KIND,
    KEEP_ALIVE_IDLE_REASON,
};
use crate::engine::thread_lifecycle::{ThreadStatus, WAITING_FOR_USER_ANSWER_EVENTS};
use crate::engine::{LucidosEngine, PreEmittedOrigin};
use crate::llm::model_prices::{cache_shares_of, CacheShares};
use crate::llm::prefill::PrefillBody;
use crate::runtime::CodingAgent;

/// The events whose waits are worth keeping warm, chosen from measured wait
/// lengths: most end within a few beats. Any other kind gets no keep-alive.
pub(crate) const KEEP_WARM_EVENT_TYPES: [&str; 7] = [
    "BackgroundBashCompleted",
    "E2ELockReleased",
    "SuiteLockReleased",
    "ChildThreadCompleted",
    "CodingAgentIdled",
    "ChangeApplied",
    "ChangeProposed",
];

/// How a beat's request reaches the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BeatStart {
    /// A Claude Code session restarts first.
    Cold,
    /// A prefill goes straight out.
    Direct,
}

impl CacheTtl {
    /// When the beat fires, in seconds since the last request started. The
    /// tick runs every 10 s. A cold beat's request lands a median 16 s and a
    /// p95 30 s later. So the 5-minute beat's lands by 270 s at p95, inside
    /// the 300 s lifetime (ADR 0492). A direct beat lands in seconds, so it
    /// fires later (ADR 0556).
    ///
    /// No Claude Code session writes a 30-minute entry, and no prefill a
    /// 1-hour one. Those two rows only keep the table whole.
    pub(super) fn beat_window(self, start: BeatStart) -> RangeInclusive<f64> {
        match (self, start) {
            (Self::FiveMinutes, BeatStart::Cold) => 230.0..=260.0,
            (Self::FiveMinutes, BeatStart::Direct) => 260.0..=280.0,
            (Self::ThirtyMinutes, BeatStart::Cold) => 1500.0..=1560.0,
            (Self::ThirtyMinutes, BeatStart::Direct) => 1560.0..=1620.0,
            (Self::OneHour, BeatStart::Cold) => 3300.0..=3360.0,
            (Self::OneHour, BeatStart::Direct) => 3500.0..=3540.0,
        }
    }

    /// How many beats cost as much as one rewrite at this TTL: the write rate
    /// over the read rate. A beat is a full-context read, and a wake after
    /// the cache expired is a full-context write.
    pub(super) fn rewrite_in_beats(self, cache: CacheShares) -> f64 {
        self.write_share(cache) / cache.read
    }

    /// The seconds between two beats.
    pub(super) fn beat_every_secs(self, start: BeatStart) -> f64 {
        *self.beat_window(start).start()
    }
}

/// The values of `cache_keep_alive_mode`, as the catalog lists them.
pub const CACHE_KEEP_ALIVE_MODES: &[&str] = &[
    CacheKeepAliveMode::Default.as_str(),
    CacheKeepAliveMode::WorkspaceData.as_str(),
    CacheKeepAliveMode::Off.as_str(),
];

/// The `cache_keep_alive_mode` preference, parsed: when a keep-alive run gives up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKeepAliveMode {
    /// Break-even for an event wait, half of it for a parked one.
    Default,
    /// The cap that would have cost least over this workspace's last 30 days.
    WorkspaceData,
    /// No beats and no status.
    Off,
}

impl CacheKeepAliveMode {
    const ALL: [Self; 3] = [Self::Default, Self::WorkspaceData, Self::Off];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::WorkspaceData => "workspace-data",
            Self::Off => "off",
        }
    }

    /// A value outside [`CACHE_KEEP_ALIVE_MODES`] reads as `default`, the
    /// catalog default.
    pub fn parse(value: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|mode| mode.as_str() == value)
            .unwrap_or(Self::Default)
    }
}

/// The kinds a keep-warm wait watches, one bit per [`KEEP_WARM_EVENT_TYPES`]
/// entry. Two waits on the same kinds are the same wait type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub(crate) struct KeepWarmKinds(u8);

impl KeepWarmKinds {
    /// The kinds of a wait watching `kinds`. `None` when it watches nothing,
    /// or a kind that is not keep-warm.
    pub(crate) fn of<'a>(kinds: impl IntoIterator<Item = &'a str>) -> Option<Self> {
        let mut bits = 0u8;
        for kind in kinds {
            let index = KEEP_WARM_EVENT_TYPES.iter().position(|k| *k == kind)?;
            bits |= 1 << index;
        }
        (bits != 0).then_some(Self(bits))
    }

    /// What working children wait for.
    fn child_finish() -> Self {
        Self::of(["ChildThreadCompleted"]).expect("a keep-warm kind")
    }
}

/// The card a parked session waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ParkedCard {
    Question,
    Permission,
}

impl ParkedCard {
    const ALL: [Self; 2] = [Self::Question, Self::Permission];

    /// The event that parks a Claude Code session on this card.
    pub(crate) const fn event_type(self) -> &'static str {
        match self {
            Self::Question => "UserQuestionAsked",
            Self::Permission => "CodingAgentPermissionRequest",
        }
    }

    /// The card `event_type` parks a session on. `None` for any other event,
    /// including the Lucidos Agent's parkings.
    pub(crate) fn of_event_type(event_type: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|card| card.event_type() == event_type)
    }
}

/// Which agent a wait belongs to. Each keeps its own wait durations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WaitAgent {
    ClaudeCode,
    Lucidos,
}

/// What a wait waits for. Waits of one type share one give-up point under
/// `workspace-data`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum WaitType {
    /// A live session parked on a card.
    Parked(ParkedCard),
    /// An event wait, or working children.
    Events(KeepWarmKinds),
}

/// What decides when a keep-alive run gives up: the preference, with what
/// `workspace-data` reads.
#[derive(Debug, Clone, Copy)]
pub(super) enum GiveUpRule<'a> {
    Default,
    /// The cheapest cap over these durations. A type with too few waits keeps
    /// the default.
    WorkspaceData(&'a WaitDurations),
    Off,
}

impl<'a> GiveUpRule<'a> {
    pub(super) fn of(mode: CacheKeepAliveMode, durations: &'a WaitDurations) -> Self {
        match mode {
            CacheKeepAliveMode::Default => Self::Default,
            CacheKeepAliveMode::WorkspaceData => Self::WorkspaceData(durations),
            CacheKeepAliveMode::Off => Self::Off,
        }
    }
}

/// How many beats a run may spend before it gives up, or `None` when it
/// spends none. [`decide`] and [`status`] both read it (I3).
fn budget_beats(c: &Candidate, rule: GiveUpRule) -> Option<f64> {
    let break_even = c.ttl().rewrite_in_beats(c.cache);
    let agent = c.agent.wait_agent()?;
    let wait_type = wait_type(c)?;
    let default = match wait_type {
        WaitType::Parked(_) => break_even / 2.0,
        WaitType::Events(_) => break_even,
    };
    match rule {
        GiveUpRule::Off => None,
        GiveUpRule::Default => Some(default),
        GiveUpRule::WorkspaceData(durations) => Some(
            durations
                .cap_beats(agent, wait_type, c.beat_every_secs(), break_even)
                .map_or(default, f64::from),
        ),
    }
}

/// How long after the wait began the engine gives up, in seconds: the budget
/// at one beat per window.
fn give_up_after_secs(c: &Candidate, rule: GiveUpRule) -> Option<f64> {
    Some(budget_beats(c, rule)? * c.beat_every_secs())
}

/// What the agent reads. It must answer without a tool call, or the turn costs
/// more than the cache it saves.
const KEEP_ALIVE_PROMPT: &str = "Cache keep-alive from the Lucidos engine. You are still \
    waiting, and nothing has arrived. Do nothing: run no command, call no tool, and send no \
    read decision. Reply with the single word: waiting.";

/// How long a copy beat may run. A beat fires by the end of its window, so a
/// copy still running a minute later has missed the cache lifetime anyway.
const KEEP_ALIVE_COPY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// A copy beat's model turns. A stray tool call then ends the copy rather than
/// paying for a second read of the whole context.
const KEEP_ALIVE_COPY_MAX_TURNS: usize = 1;

/// The in-memory guard against a second beat while the first one's request is
/// still on its way. Lost on restart, which is harmless: a beat in flight then
/// died with its session.
pub(crate) type KeepAliveBeats = Arc<std::sync::Mutex<HashMap<Uuid, std::time::Instant>>>;

/// Whether anything else has the thread right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ThreadClaim {
    Free,
    Busy,
}

/// What the engine knows about one waiting thread at a tick.
#[derive(Debug, Clone)]
pub(super) struct Candidate {
    pub agent: Agent,
    pub claim: ThreadClaim,
    /// A person owes the thread an answer, so a turn would supersede the card.
    pub awaiting_user: bool,
    /// The card it waits on, when its latest parking was a question card or
    /// a Claude Code permission prompt.
    pub parked: Option<Parked>,
    /// Seconds since the newest of its live waits that watch keep-warm kinds
    /// only was armed. `None` when it holds no such wait.
    pub keep_warm_wait_secs: Option<f64>,
    /// The kinds that newest wait watches.
    pub keep_warm_kinds: KeepWarmKinds,
    pub children: ActiveChildren,
    /// Seconds since the last turn request started. `None` when it never ran.
    pub secs_since_request: Option<f64>,
    /// Seconds since this engine last beat for the thread, from the cache.
    pub secs_since_beat: Option<f64>,
    /// The TTL its latest cache write showed. `None` when none showed one.
    pub cache_ttl: Option<CacheTtl>,
    /// What the prompt cache of the model that wrote it charges.
    pub cache: CacheShares,
    /// When the user turned the keep-alive off for this thread, while that
    /// switch holds: no real request has started since.
    pub turned_off_at: Option<DateTime<Utc>>,
}

/// The agent that runs a thread, as the keep-alive weighs it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Agent {
    ClaudeCode,
    /// Never beaten (ADR 0470).
    Codex,
    /// The Lucidos Agent, with what the engine remembers of its last request.
    Lucidos(Remembered),
}

impl Agent {
    fn beat_start(self) -> BeatStart {
        match self {
            Self::Lucidos(_) => BeatStart::Direct,
            Self::ClaudeCode | Self::Codex => BeatStart::Cold,
        }
    }

    fn wait_agent(self) -> Option<WaitAgent> {
        match self {
            Self::ClaudeCode => Some(WaitAgent::ClaudeCode),
            Self::Lucidos(_) => Some(WaitAgent::Lucidos),
            Self::Codex => None,
        }
    }
}

/// A turn parked on a card. It blocks on the engine's reply, so the beat goes
/// beside it: a copy of a Claude Code session, or a Lucidos Agent prefill.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Parked {
    pub card: ParkedCard,
    /// Seconds since the turn request that asked started. The turn never
    /// idles while parked, so the wait's clock starts there.
    pub secs: f64,
    /// The engine remembers what the session's spawn appended. Without it a
    /// copy misses the session's cached prefix. Only Claude Code reads it.
    pub copy_ready: bool,
}

/// The thread's active child threads, as the keep-alive weighs them. An active
/// child is `running` or `waiting_for_user_answer`, and not `no-wake`. Only a
/// `running` one works.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum ActiveChildren {
    None,
    /// Every active child waits on a question, permission or plan approval card.
    AllAwaitingUser,
    /// At least one active child is working. The wait began when the agent
    /// last went idle for real, this many seconds ago.
    Working {
        wait_secs: f64,
    },
    /// Only `no-wake` children work. None of them would wake the thread.
    OnlyNoWake,
}

/// The selection's verdict.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Beat {
    /// Beat now, by this route.
    Send(BeatRoute),
    Skip(SkipReason),
}

/// How a beat reaches the session's cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BeatRoute {
    /// Resume the idle session with the hidden prompt.
    Resume,
    /// Ask a copy of the parked session, which reads the same cached prefix.
    /// The parked session itself never hears of it (ADR 0082).
    SessionCopy,
    /// Send the Lucidos Agent's last request again, with no output.
    Prefill(PrefillCut),
}

/// Which cut of the remembered request a prefill beat sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PrefillCut {
    /// The stable prefix, which the next turn sends again.
    Stable,
    /// The whole request, which the answer to its own card extends.
    Whole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SkipReason {
    Codex,
    /// The `cache_keep_alive_mode` preference is `off`.
    Disabled,
    Busy,
    /// A person owes an answer, and no live session holds the thread.
    AwaitingUser,
    /// Parked, but a copy would miss the cache: no appended prompt is known.
    NoCopyPrompt,
    ChildrenAwaitingUser,
    OnlyNoWakeChildren,
    NothingToWaitFor,
    PastCutOff,
    NoRequestYet,
    OutsideWindow,
    BeatInFlight,
    /// The user turned the keep-alive off for this thread.
    TurnedOff,
    /// The provider cannot prefill the thread's last request.
    UncacheableProvider,
    /// The engine remembers no request for the thread since it restarted.
    Restarted,
}

/// Decide one thread's beat. Pure, so every condition and boundary is
/// table-tested (I5).
pub(super) fn decide(c: &Candidate, rule: GiveUpRule) -> Beat {
    use SkipReason::*;
    let route = match route(c) {
        Ok(route) => route,
        Err(reason) => return Beat::Skip(reason),
    };
    let waiting_secs = match waiting_secs(c) {
        Ok(secs) => secs,
        Err(reason) => return Beat::Skip(reason),
    };
    if c.turned_off_at.is_some() {
        return Beat::Skip(TurnedOff);
    }
    let Some(give_up_after) = give_up_after_secs(c, rule) else {
        return Beat::Skip(Disabled);
    };
    if waiting_secs > give_up_after {
        return Beat::Skip(PastCutOff);
    }
    if let Some(reason) = forgotten(c) {
        return Beat::Skip(reason);
    }
    let Some(secs) = c.secs_since_request else {
        return Beat::Skip(NoRequestYet);
    };
    let window = c.beat_window();
    if !window.contains(&secs) {
        return Beat::Skip(OutsideWindow);
    }
    if c.secs_since_beat.is_some_and(|s| s < *window.start()) {
        return Beat::Skip(BeatInFlight);
    }
    Beat::Send(route)
}

/// How a beat would reach the thread's cache, or why none can now.
fn route(c: &Candidate) -> Result<BeatRoute, SkipReason> {
    use SkipReason::*;
    let parked_on_question = matches!(
        c.parked,
        Some(Parked {
            card: ParkedCard::Question,
            ..
        })
    );
    match (c.agent, c.claim, c.awaiting_user, c.parked) {
        (Agent::Codex, ..) => Err(Codex),
        (Agent::ClaudeCode, ThreadClaim::Busy, true, Some(parked)) if !parked.copy_ready => {
            Err(NoCopyPrompt)
        }
        (Agent::ClaudeCode, ThreadClaim::Busy, true, Some(_)) => Ok(BeatRoute::SessionCopy),
        (Agent::Lucidos(_), ThreadClaim::Busy, true, _) if parked_on_question => {
            Ok(BeatRoute::Prefill(PrefillCut::Whole))
        }
        (_, ThreadClaim::Busy, _, _) => Err(Busy),
        (_, ThreadClaim::Free, true, _) => Err(AwaitingUser),
        (Agent::ClaudeCode, ThreadClaim::Free, false, _) => Ok(BeatRoute::Resume),
        (Agent::Lucidos(_), ThreadClaim::Free, false, _) => {
            Ok(BeatRoute::Prefill(PrefillCut::Stable))
        }
    }
}

/// Why a Lucidos Agent thread has no request to beat with.
fn forgotten(c: &Candidate) -> Option<SkipReason> {
    match c.agent {
        Agent::Lucidos(Remembered::Uncacheable) => Some(SkipReason::UncacheableProvider),
        Agent::Lucidos(Remembered::Nothing) => Some(SkipReason::Restarted),
        Agent::Lucidos(Remembered::Pair(_)) | Agent::ClaudeCode | Agent::Codex => None,
    }
}

/// The wait the keep-alive measures: how long it has run and its type, or why
/// the thread waits for nothing worth keeping warm.
fn measured_wait(c: &Candidate) -> Result<(f64, WaitType), SkipReason> {
    if let Some(parked) = parked_wait(c) {
        return Ok((parked.secs, WaitType::Parked(parked.card)));
    }
    let events = WaitType::Events(c.keep_warm_kinds);
    let children = WaitType::Events(KeepWarmKinds::child_finish());
    // The most recent start wins: a fresh wait keeps a thread warm even when
    // its children have run for hours.
    match (c.keep_warm_wait_secs, c.children) {
        (Some(a), ActiveChildren::Working { wait_secs: b }) if b < a => Ok((b, children)),
        (Some(a), _) => Ok((a, events)),
        (None, ActiveChildren::Working { wait_secs }) => Ok((wait_secs, children)),
        (None, ActiveChildren::AllAwaitingUser) => Err(SkipReason::ChildrenAwaitingUser),
        (None, ActiveChildren::OnlyNoWake) => Err(SkipReason::OnlyNoWakeChildren),
        (None, ActiveChildren::None) => Err(SkipReason::NothingToWaitFor),
    }
}

/// The card the thread is parked on, while a person owes it an answer.
fn parked_wait(c: &Candidate) -> Option<Parked> {
    c.parked.filter(|_| c.awaiting_user)
}

fn waiting_secs(c: &Candidate) -> Result<f64, SkipReason> {
    measured_wait(c).map(|(secs, _)| secs)
}

pub(super) fn wait_type(c: &Candidate) -> Option<WaitType> {
    measured_wait(c).ok().map(|(_, wait_type)| wait_type)
}

impl Candidate {
    /// A remembered request names its provider's TTL. Otherwise an unknown
    /// TTL beats as 5 minutes. Beating a 1-hour cache that often wastes
    /// reads, but beating a 5-minute one an hour late loses it.
    fn ttl(&self) -> CacheTtl {
        match self.agent {
            Agent::Lucidos(Remembered::Pair(ttl)) => ttl,
            _ => self.cache_ttl.unwrap_or(CacheTtl::FiveMinutes),
        }
    }

    fn beat_window(&self) -> RangeInclusive<f64> {
        self.ttl().beat_window(self.agent.beat_start())
    }

    fn beat_every_secs(&self) -> f64 {
        self.ttl().beat_every_secs(self.agent.beat_start())
    }
}

/// When the times [`status`] reports began, as the database stamped them. The
/// ages in [`Candidate`] drift by a few milliseconds per tick, so a status built
/// from them would change on every tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Anchors {
    /// When the newest keep-warm wait was armed.
    pub keep_warm_armed_at: Option<DateTime<Utc>>,
    /// When the agent last went idle for real.
    pub idle_at: Option<DateTime<Utc>>,
    /// When the last turn request started.
    pub request_started_at: Option<DateTime<Utc>>,
    /// When the last turn request started, ignoring copy beats. A parked wait
    /// began there.
    pub turn_started_at: Option<DateTime<Utc>>,
}

/// What the current keep-alive run has spent, read from persisted events.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(super) struct Spent {
    pub beats: u32,
    pub cost_usd: f64,
    /// The context of the session's last turn request.
    pub context_tokens: u64,
}

/// The *cache keep-alive status* of one thread. Pure, and it reads the same
/// inputs as [`decide`], so the two agree on every candidate (I3). A thread
/// claim never changes it: [`sync_status`] leaves a busy thread's status alone.
/// A parked thread shows none, so no exchange opens beside its card (ADR 0540).
pub(super) fn status(
    c: &Candidate,
    rule: GiveUpRule,
    at: &Anchors,
    spent: Spent,
) -> Option<CacheKeepAliveStatus> {
    if c.agent == Agent::Codex || c.awaiting_user || matches!(rule, GiveUpRule::Off) {
        return None;
    }
    let waiting_secs = match waiting_secs(c) {
        Ok(_) if c.turned_off_at.is_some() => {
            return Some(CacheKeepAliveStatus::NotKept {
                reason: CacheKeepAliveNotKeptReason::TurnedOff,
            })
        }
        Ok(secs) => secs,
        Err(SkipReason::ChildrenAwaitingUser) => {
            return Some(CacheKeepAliveStatus::NotKept {
                reason: CacheKeepAliveNotKeptReason::ChildrenAwaitingUser,
            })
        }
        Err(SkipReason::OnlyNoWakeChildren) => {
            return Some(CacheKeepAliveStatus::NotKept {
                reason: CacheKeepAliveNotKeptReason::NoWakeChildren,
            })
        }
        Err(_) => return None,
    };
    let since = wait_began_at(c, at)?;
    let budget = budget_beats(c, rule)?;
    let give_up_after = give_up_after_secs(c, rule)?;
    let give_up_at = since + chrono::Duration::milliseconds((give_up_after * 1000.0) as i64);
    if waiting_secs > give_up_after {
        return Some(CacheKeepAliveStatus::Stopped {
            at: give_up_at,
            beats: spent.beats,
            cost_usd: spent.cost_usd,
            context_tokens: spent.context_tokens,
        });
    }
    let not_kept = |reason| Some(CacheKeepAliveStatus::NotKept { reason });
    match forgotten(c) {
        Some(SkipReason::UncacheableProvider) => {
            return not_kept(CacheKeepAliveNotKeptReason::UncacheableProvider)
        }
        Some(_) => return not_kept(CacheKeepAliveNotKeptReason::Restarted),
        None => {}
    }
    let beat_every_secs = c.beat_every_secs();
    let window_start = chrono::Duration::milliseconds((beat_every_secs * 1000.0) as i64);
    Some(CacheKeepAliveStatus::Warming {
        since,
        beats: spent.beats,
        budget_beats: budget.floor() as u32,
        beat_every_secs: beat_every_secs as u32,
        cost_usd: spent.cost_usd,
        context_tokens: spent.context_tokens,
        next_beat_at: at.request_started_at.map(|t| t + window_start),
        give_up_at,
    })
}

/// When the wait [`waiting_secs`] measures began: the later start wins there,
/// so it wins here.
fn wait_began_at(c: &Candidate, at: &Anchors) -> Option<DateTime<Utc>> {
    match (c.keep_warm_wait_secs, c.children) {
        (Some(_), ActiveChildren::Working { .. }) => at.keep_warm_armed_at.max(at.idle_at),
        (Some(_), _) => at.keep_warm_armed_at,
        (None, ActiveChildren::Working { .. }) => at.idle_at,
        (None, _) => None,
    }
}

/// What the keep-alive does for a thread, as the *cache-warm line* explains it.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum WarmLineKeepAlive {
    /// Beats keep the parked thread warm until `until`.
    Kept {
        route: WarmLineRoute,
        until: DateTime<Utc>,
        beats: u32,
        cost_usd: f64,
        context_tokens: u64,
    },
    /// The beats stopped at `at`: more would cost more than a rewrite.
    Stopped {
        at: DateTime<Utc>,
        beats: u32,
        cost_usd: f64,
        context_tokens: u64,
    },
    NotKept {
        reason: WarmLineNotKept,
    },
    /// The user turned the keep-alive off for this thread at `at`. A reply
    /// after the cache goes cold rereads `context_tokens`.
    TurnedOff {
        at: DateTime<Utc>,
        context_tokens: u64,
    },
}

/// Why nothing keeps a thread's cache warm while it waits on a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WarmLineNotKept {
    /// A Codex thread, which is never beaten.
    Codex,
    /// The provider cannot prefill the Lucidos Agent's last request.
    UncacheableProvider,
    /// The `cache_keep_alive_mode` preference is `off`.
    Off,
    /// Lucidos restarted since the thread's last request, or its Claude Code
    /// session ended, so a beat would miss the cached prefix.
    Restarted,
    /// No card is parked on the user. An event wait has its own keep-alive card.
    NothingPending,
}

/// How the keep-alive beats a thread parked on its card, which the line's
/// explainer names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WarmLineRoute {
    /// A throwaway copy of the Claude Code session answers one word.
    Copy,
    /// The start of the Lucidos Agent's last request goes out again.
    Prefill,
}

/// The keep-alive's answer for the cache-warm line. Pure, and it reads the
/// inputs [`decide`] reads, so the line says `Kept` exactly while a copy beat
/// is due: `until` is the give-up time `decide` stops at (I3).
pub(super) fn cache_warm_line(
    c: &Candidate,
    rule: GiveUpRule,
    at: &Anchors,
    spent: Spent,
) -> WarmLineKeepAlive {
    let not_kept = |reason| WarmLineKeepAlive::NotKept { reason };
    if c.agent == Agent::Codex {
        return not_kept(WarmLineNotKept::Codex);
    }
    if matches!(rule, GiveUpRule::Off) {
        return not_kept(WarmLineNotKept::Off);
    }
    if let Some(at) = c.turned_off_at {
        return WarmLineKeepAlive::TurnedOff {
            at,
            context_tokens: spent.context_tokens,
        };
    }
    let (Some(parked), Some(give_up_after), Some(since)) = (
        parked_wait(c),
        give_up_after_secs(c, rule),
        at.turn_started_at,
    ) else {
        return not_kept(WarmLineNotKept::NothingPending);
    };
    let past_budget = parked.secs > give_up_after;
    let beside_the_turn = c.claim == ThreadClaim::Busy
        && match c.agent {
            Agent::ClaudeCode => parked.copy_ready,
            // The tick forgets the pair at give-up, so a stopped run
            // remembers nothing.
            Agent::Lucidos(remembered) => past_budget || remembered != Remembered::Nothing,
            Agent::Codex => false,
        };
    if !beside_the_turn {
        return not_kept(WarmLineNotKept::Restarted);
    }
    if c.agent == Agent::Lucidos(Remembered::Uncacheable) {
        return not_kept(WarmLineNotKept::UncacheableProvider);
    }
    let give_up_at = since + chrono::Duration::milliseconds((give_up_after * 1000.0) as i64);
    let Spent {
        beats,
        cost_usd,
        context_tokens,
    } = spent;
    if past_budget {
        WarmLineKeepAlive::Stopped {
            at: give_up_at,
            beats,
            cost_usd,
            context_tokens,
        }
    } else {
        WarmLineKeepAlive::Kept {
            route: match c.agent {
                Agent::Lucidos(_) => WarmLineRoute::Prefill,
                Agent::ClaudeCode | Agent::Codex => WarmLineRoute::Copy,
            },
            until: give_up_at,
            beats,
            cost_usd,
            context_tokens,
        }
    }
}

impl WarmLineKeepAlive {
    /// The context a reply after the cache goes cold rereads, where the line
    /// names it.
    fn reread_tokens(self) -> Option<u64> {
        match self {
            Self::Kept { context_tokens, .. }
            | Self::Stopped { context_tokens, .. }
            | Self::TurnedOff { context_tokens, .. } => {
                Some(context_tokens).filter(|tokens| *tokens > 0)
            }
            Self::NotKept { .. } => None,
        }
    }
}

/// The cache-warm line: what the keep-alive does, and what a reply after the
/// cache goes cold pays to reread.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize)]
pub(crate) struct WarmLine {
    pub keep_alive: WarmLineKeepAlive,
    pub reread_usd: Option<f64>,
}

/// What rewriting `tokens` to a `ttl` cache costs at `model`'s list price
/// today. `None` for an unpriced model.
pub(super) fn reread_usd(tokens: u64, model: &str, ttl: CacheTtl) -> Option<f64> {
    let tokens = u32::try_from(tokens).unwrap_or(u32::MAX);
    let usage = crate::engine::ApiUsage {
        input_tokens: tokens,
        cache_creation_tokens: tokens,
        cache_creation_1h_tokens: (ttl == CacheTtl::OneHour).then_some(tokens),
        ..Default::default()
    };
    usage
        .usd_on(model, chrono::Local::now().date_naive())
        .map(round_usd)
}

/// The newest of `waits` that watch keep-warm kinds only: when it was armed,
/// and what it watches.
pub(super) fn newest_keep_warm_wait<'a>(
    waits: impl IntoIterator<Item = &'a LiveWait>,
) -> Option<(DateTime<Utc>, KeepWarmKinds)> {
    waits
        .into_iter()
        .filter_map(|w| {
            let kinds = KeepWarmKinds::of(w.on.iter().map(|s| s.event_type.as_str()))?;
            Some((w.armed_at, kinds))
        })
        .max_by_key(|(armed_at, _)| *armed_at)
}

/// One thread's row, as the tick reads it.
#[derive(sqlx::FromRow)]
struct CandidateRow {
    thread_id: Uuid,
    is_coding_agent: bool,
    coding_agent: Option<String>,
    awaiting_user: bool,
    has_active_children: bool,
    has_working_children: bool,
    has_no_wake_working_children: bool,
    /// The event of its latest parking, when it waits on the user.
    parked_by: Option<String>,
    /// Since the last refresh: a turn request or a copy beat.
    secs_since_request: Option<f64>,
    request_started_at: Option<DateTime<Utc>>,
    turn_started_at: Option<DateTime<Utc>>,
    secs_since_turn_request: Option<f64>,
    secs_since_idle: Option<f64>,
    idle_at: Option<DateTime<Utc>>,
    last_write_tokens: Option<i64>,
    last_write_1h_tokens: Option<i64>,
    last_write_model: Option<String>,
    stored_status: Option<sqlx::types::Json<CacheKeepAliveStatus>>,
    /// When the user turned the keep-alive off, while that switch holds.
    turned_off_at: Option<DateTime<Utc>>,
}

impl CandidateRow {
    fn cache_ttl(&self) -> Option<CacheTtl> {
        CacheTtl::observed(self.last_write_tokens?, self.last_write_1h_tokens)
    }

    /// The prompt cache prices of the model that wrote the latest cache entry.
    /// A session with no write yet, or an unpriced model, gets the standard
    /// shares.
    fn cache(&self) -> CacheShares {
        cache_shares_of(self.last_write_model.as_deref().unwrap_or_default())
    }

    /// The card its session is parked on. `None` when its latest parking was
    /// not a Claude Code one.
    fn parked_card(&self) -> Option<ParkedCard> {
        ParkedCard::of_event_type(self.parked_by.as_deref()?)
    }

    fn active_children(&self) -> ActiveChildren {
        match (
            self.has_active_children,
            self.has_working_children,
            self.secs_since_idle,
        ) {
            (false, _, _) if self.has_no_wake_working_children => ActiveChildren::OnlyNoWake,
            (false, _, _) => ActiveChildren::None,
            (true, false, _) => ActiveChildren::AllAwaitingUser,
            (true, true, Some(wait_secs)) => ActiveChildren::Working { wait_secs },
            // The agent never went idle for real, so no wait began.
            (true, true, None) => ActiveChildren::None,
        }
    }
}

/// Every thread that holds a live wait or active children, and every
/// non-Codex thread parked on the user, with what the selection reads. Ages of
/// database rows are computed in SQL (ADR 0053).
///
/// - The latest real idle is the latest `CodingAgentIdled` no keep-alive turn
///   wrote, or for a Lucidos Agent thread its latest `ResponseGenerated`. A
///   prefill beat writes neither. [`read_candidates`] binds the reason.
/// - The last refresh is [`last_cache_refresh_started_sql`], the one
///   definition the cache-warm line shares.
/// - The per-thread switch holds while it is later than
///   [`last_real_request_started_sql`]. CASE evaluates that look-up only for
///   a thread that has the switch set.
/// - The TTL and the model come from the latest capture of the session's own
///   that wrote to the cache. A cache hit keeps an entry's lifetime, and a
///   sub-agent may run on another TTL or model. A Codex thread never writes
///   one, so it skips the look-up rather than scan its whole history.
fn candidates_sql() -> String {
    let turn_started = last_turn_started_sql("ts.thread_id");
    let refresh_started = last_cache_refresh_started_sql("ts.thread_id");
    let real_request_started = last_real_request_started_sql("ts.thread_id");
    // A handed-off session is ending, so there is no cache worth keeping.
    let handoff_pending = crate::engine::successor::pending_handoff_sql("ts.thread_id");
    let in_session =
        crate::engine::session_scope::in_current_session_sql("ts.thread_id", "e.sequence");
    format!(
        r#"
SELECT ts.thread_id,
       ts.is_coding_agent,
       ts.coding_agent,
       ts.status = $3 AS awaiting_user,
       p.event_type AS parked_by,
       EXISTS (SELECT 1 FROM thread_summaries c
                WHERE c.parent_thread_id = ts.thread_id
                  AND c.wake_mode IS DISTINCT FROM $6
                  AND c.status IN ($3, $4)) AS has_active_children,
       EXISTS (SELECT 1 FROM thread_summaries c
                WHERE c.parent_thread_id = ts.thread_id
                  AND c.wake_mode IS DISTINCT FROM $6
                  AND c.status = $4) AS has_working_children,
       EXISTS (SELECT 1 FROM thread_summaries c
                WHERE c.parent_thread_id = ts.thread_id
                  AND c.wake_mode = $6
                  AND c.status = $4) AS has_no_wake_working_children,
       EXTRACT(EPOCH FROM now() - r.refresh_started)::float8 AS secs_since_request,
       r.refresh_started AS request_started_at,
       r.turn_started AS turn_started_at,
       EXTRACT(EPOCH FROM now() - r.turn_started)::float8 AS secs_since_turn_request,
       EXTRACT(EPOCH FROM now() - i.created)::float8 AS secs_since_idle,
       i.created AS idle_at,
       w.written AS last_write_tokens,
       w.written_1h AS last_write_1h_tokens,
       w.model AS last_write_model,
       ts.cache_keep_alive_status AS stored_status,
       CASE WHEN ts.cache_keep_alive_off_at IS NULL THEN NULL
            WHEN ts.cache_keep_alive_off_at
                 > COALESCE({real_request_started}, '-infinity'::timestamptz)
            THEN ts.cache_keep_alive_off_at END AS turned_off_at
  FROM thread_summaries ts
  LEFT JOIN LATERAL (
       SELECT {turn_started} AS turn_started,
              {refresh_started} AS refresh_started) r ON true
  LEFT JOIN LATERAL (
       SELECT e.event_type
         FROM events e
        WHERE ts.status = $3
          AND e.thread_id = ts.thread_id
          AND e.event_type = ANY($7)
        ORDER BY e.sequence DESC LIMIT 1) p ON true
  LEFT JOIN LATERAL (
       SELECT e.created
         FROM events e
        WHERE e.thread_id = ts.thread_id
          AND e.event_type = CASE WHEN ts.is_coding_agent
                                  THEN 'CodingAgentIdled' ELSE 'ResponseGenerated' END
          AND COALESCE(e.payload->>'reason', '') <> $2
        ORDER BY e.sequence DESC LIMIT 1) i ON true
  LEFT JOIN LATERAL (
       SELECT (e.payload->'usage'->>'cache_creation_tokens')::bigint AS written,
              (e.payload->'usage'->>'cache_creation_1h_tokens')::bigint AS written_1h,
              e.payload->>'model' AS model
         FROM events e
        WHERE ts.coding_agent IS DISTINCT FROM $5
          AND e.thread_id = ts.thread_id
          AND e.event_type = 'ContextCaptured'
          AND COALESCE(e.payload->>'purpose', 'turn') = 'turn'
          AND e.payload->>'parent_tool_use_id' IS NULL
          AND (e.payload->'usage'->>'cache_creation_tokens')::bigint > 0
          AND {in_session}
        ORDER BY e.sequence DESC LIMIT 1) w ON true
 WHERE NOT {handoff_pending}
   AND (ts.thread_id = ANY($1)
        OR ts.active_children_count > 0
        OR (ts.status = $3 AND ts.coding_agent IS DISTINCT FROM $5))
"#
    )
}

/// What the pings since `$2` spent. A call may record several frames, so each
/// call counts once, at its last frame, priced on the day of that frame. `$2` is the wait start, from the engine's clock or the database's.
/// The first ping comes minutes later, so their drift moves no ping across it.
const SPENT_SQL: &str = r#"
WITH anchors AS (
     SELECT e.id::text AS id
       FROM events e
      WHERE e.thread_id = $1
        AND e.event_type = 'PromptInjected'
        AND e.payload->'origin'->'reason'->>'kind' = $3
        AND e.created >= $2)
SELECT (SELECT count(*) FROM anchors) AS beats,
       COALESCE((
         SELECT jsonb_agg(jsonb_build_object('model', c.model, 'usage', c.usage, 'created', c.created))
           FROM (SELECT DISTINCT ON (COALESCE(e.payload->>'api_call_id', e.id::text))
                        e.payload->>'model' AS model, e.payload->'usage' AS usage,
                        e.created
                   FROM events e
                  WHERE e.thread_id = $1
                    AND e.event_type = 'ContextCaptured'
                    AND e.payload->>'request_event_id' IN (SELECT id FROM anchors)
                    AND e.payload->'usage' IS NOT NULL
                  ORDER BY COALESCE(e.payload->>'api_call_id', e.id::text), e.sequence DESC) c
       ), '[]'::jsonb) AS calls
"#;

/// One priced call of a ping turn.
#[derive(serde::Deserialize)]
struct SpentCall {
    model: Option<String>,
    usage: crate::engine::ApiUsage,
    created: DateTime<Utc>,
}

/// Micro-dollars: a summed `f64` and its JSONB round trip then compare equal,
/// so an unchanged status is never rewritten (I4).
fn round_usd(usd: f64) -> f64 {
    (usd * 1e6).round() / 1e6
}

/// What the current run spent: a Lucidos Agent's beats are captures, and a
/// Claude Code resume beat is a turn behind an anchor.
async fn read_run_spent(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    agent: Agent,
    since: DateTime<Utc>,
) -> Result<Spent, sqlx::Error> {
    match agent {
        Agent::Lucidos(_) => read_capture_spent(pool, thread_id, since).await,
        Agent::ClaudeCode | Agent::Codex => read_spent(pool, thread_id, since).await,
    }
}

async fn read_spent(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Spent, sqlx::Error> {
    let (beats, calls): (i64, sqlx::types::Json<Vec<SpentCall>>) = sqlx::query_as(SPENT_SQL)
        .bind(thread_id)
        .bind(since)
        .bind(CACHE_KEEP_ALIVE_REASON_KIND)
        .fetch_one(pool)
        .await?;
    let context = crate::engine::successor::thread_context(pool, thread_id).await?;
    Ok(Spent {
        beats: u32::try_from(beats).unwrap_or(u32::MAX),
        cost_usd: price_calls(&calls.0),
        context_tokens: context.input_tokens.unwrap_or(0),
    })
}

/// What `calls` cost, each priced on the day it ran.
fn price_calls(calls: &[SpentCall]) -> f64 {
    round_usd(
        calls
            .iter()
            .filter_map(|call| {
                let day = call.created.with_timezone(&chrono::Local).date_naive();
                call.usage
                    .usd_on(call.model.as_deref().unwrap_or_default(), day)
            })
            .sum(),
    )
}

/// The beats since `$2` that record a capture of their own (a session copy
/// or a prefill): one `cache_keep_alive` call each, with its model, usage and
/// time. A call may record several frames, so each counts once, at its last
/// frame. `$1` is the thread and `$3` the purpose.
const BEAT_CAPTURES_SQL: &str = r#"
SELECT COALESCE(jsonb_agg(jsonb_build_object(
           'model', c.model, 'usage', c.usage, 'created', c.created)
         ORDER BY c.sequence), '[]'::jsonb)
  FROM (SELECT DISTINCT ON (COALESCE(e.payload->>'api_call_id', e.id::text))
               e.payload->>'model' AS model, e.payload->'usage' AS usage,
               e.created, e.sequence
          FROM events e
         WHERE e.thread_id = $1
           AND e.event_type = 'ContextCaptured'
           AND e.payload->>'purpose' = $3
           AND e.payload->'usage' IS NOT NULL
           AND e.created >= $2
         ORDER BY COALESCE(e.payload->>'api_call_id', e.id::text), e.sequence DESC) c
"#;

/// What the captured beats since `since` spent, and the context a reply after
/// the last beat would reread. A copy or a prefill beat writes no anchor, so
/// its spend comes from the captures.
async fn read_capture_spent(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    since: DateTime<Utc>,
) -> Result<Spent, sqlx::Error> {
    let calls: sqlx::types::Json<Vec<SpentCall>> = sqlx::query_scalar(BEAT_CAPTURES_SQL)
        .bind(thread_id)
        .bind(since)
        .bind(crate::engine::ContextPurpose::CacheKeepAlive.wire())
        .fetch_one(pool)
        .await?;
    let context = crate::engine::successor::thread_context(pool, thread_id).await?;
    Ok(Spent {
        beats: u32::try_from(calls.0.len()).unwrap_or(u32::MAX),
        cost_usd: price_calls(&calls.0),
        context_tokens: context.input_tokens.unwrap_or(0),
    })
}

/// The threads holding a status that are no longer candidates.
const STALE_STATUS_SQL: &str = "SELECT thread_id FROM thread_summaries \
     WHERE cache_keep_alive_status IS NOT NULL AND NOT (thread_id = ANY($1))";

/// Write the thread's *cache keep-alive status* when it changed (I4). A busy
/// thread keeps the one it has: a beat's own turn holds the thread, and every
/// call of a real turn moves the request anchor. A parked session holds the
/// thread too, but shows no status.
async fn sync_status(
    bus: &EventBus,
    thread_id: Uuid,
    candidate: &Candidate,
    rule: GiveUpRule<'_>,
    anchors: &Anchors,
    stored: Option<CacheKeepAliveStatus>,
) {
    if candidate.claim == ThreadClaim::Busy && !candidate.awaiting_user {
        return;
    }
    let spent = match wait_began_at(candidate, anchors) {
        Some(since) => match read_run_spent(bus.pool(), thread_id, candidate.agent, since).await {
            Ok(spent) => spent,
            Err(e) => {
                crate::log!("[KeepAlive] Could not read the pings of thread {thread_id}: {e}");
                return;
            }
        },
        None => Spent::default(),
    };
    let next = status(candidate, rule, anchors, spent);
    if next != stored {
        emit_status(bus, thread_id, next).await;
    }
}

/// Clear the status of every thread that stopped being a candidate, so a
/// finished wait or a restart leaves none behind (I6).
async fn clear_stale_statuses(bus: &EventBus, candidates: &[Uuid]) {
    let stale: Vec<Uuid> = match sqlx::query_scalar(STALE_STATUS_SQL)
        .bind(candidates)
        .fetch_all(bus.pool())
        .await
    {
        Ok(stale) => stale,
        Err(e) => {
            crate::log!("[KeepAlive] Could not read the stale statuses: {e}");
            return;
        }
    };
    for thread_id in stale {
        emit_status(bus, thread_id, None).await;
    }
}

async fn emit_status(bus: &EventBus, thread_id: Uuid, status: Option<CacheKeepAliveStatus>) {
    let event = ThreadEvent::CacheKeepAliveStatusChanged { status };
    if let Err(e) = bus
        .emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta::NONE,
        })
        .await
    {
        crate::log!("[KeepAlive] Could not write the status of thread {thread_id}: {e}");
    }
}

/// The candidates: `waiting` threads, every parent with active children, and
/// every thread parked on the user.
async fn read_candidates(
    pool: &sqlx::PgPool,
    waiting: &[Uuid],
) -> Result<Vec<CandidateRow>, sqlx::Error> {
    sqlx::query_as(&candidates_sql())
        .bind(waiting)
        .bind(KEEP_ALIVE_IDLE_REASON)
        .bind(ThreadStatus::WaitingForUserAnswer.as_str())
        .bind(ThreadStatus::Running.as_str())
        .bind(CodingAgent::Codex.as_str())
        .bind(WakeMode::NoWake.as_str())
        .bind(WAITING_FOR_USER_ANSWER_EVENTS)
        .fetch_all(pool)
        .await
}

/// The user's per-thread keep-alive switch, as a request names it (ADR 0554).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum KeepAliveSwitch {
    Off,
    On,
}

impl LucidosEngine {
    /// Turn the keep-alive off or back on for `thread_id` alone. The next tick
    /// reads the projected column, so a run stops or resumes within one tick.
    pub(crate) async fn switch_cache_keep_alive(
        &self,
        thread_id: Uuid,
        switch: KeepAliveSwitch,
        meta: EventMeta,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let event = match switch {
            KeepAliveSwitch::Off => ThreadEvent::CacheKeepAliveTurnedOff,
            KeepAliveSwitch::On => ThreadEvent::CacheKeepAliveTurnedOn,
        };
        self.event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event,
                meta,
            })
            .await?;
        Ok(())
    }

    /// One tick of the keep-alive: beat every thread the selection picks.
    /// Runs on the event-wait deadline sweep.
    pub(super) async fn beat_cache_keep_alives(self: &Arc<Self>) {
        // Host clock against host clock: an engine stamps `armed_at`.
        let now = Utc::now();
        let snapshot = self.live_waits.snapshot().await;
        let mut by_thread: HashMap<Uuid, Vec<&LiveWait>> = HashMap::new();
        for wait in &snapshot {
            by_thread.entry(wait.thread_id).or_default().push(wait);
        }
        let newest: HashMap<Uuid, (DateTime<Utc>, KeepWarmKinds)> = by_thread
            .into_iter()
            .filter_map(|(thread_id, waits)| Some((thread_id, newest_keep_warm_wait(waits)?)))
            .collect();
        let waiting: Vec<Uuid> = newest.keys().copied().collect();
        let mode = self.keep_alive_mode().await;
        if mode == CacheKeepAliveMode::WorkspaceData {
            self.refresh_wait_durations_when_due();
        }
        let durations = self.keep_alive_durations.current();
        let rule = GiveUpRule::of(mode, &durations);
        let rows = match read_candidates(&self.pool, &waiting).await {
            Ok(rows) => rows,
            Err(e) => {
                crate::log!("[KeepAlive] Could not read the waiting threads: {e}");
                return;
            }
        };
        let candidates: Vec<Uuid> = rows.iter().map(|row| row.thread_id).collect();
        self.remembered_requests.prune(
            &candidates.iter().copied().collect(),
            std::time::Instant::now(),
        );
        for row in rows {
            let wait = newest.get(&row.thread_id).copied();
            let candidate = self.candidate_of(&row, wait, now).await;
            match decide(&candidate, rule) {
                // The beat's own turn now holds the thread, so the next idle
                // tick writes the status, with the ping priced.
                Beat::Send(BeatRoute::Resume) => {
                    self.send_keep_alive(row.thread_id).await;
                    continue;
                }
                Beat::Send(BeatRoute::SessionCopy) => {
                    self.send_keep_alive_copy(row.thread_id);
                    continue;
                }
                // A prefill never holds the thread, so the status syncs now.
                Beat::Send(BeatRoute::Prefill(cut)) => self.send_prefill_beat(row.thread_id, cut),
                Beat::Skip(SkipReason::PastCutOff) => {
                    self.remembered_requests.forget(row.thread_id)
                }
                Beat::Skip(_) => {}
            }
            let anchors = Anchors {
                keep_warm_armed_at: wait.map(|(at, _)| at),
                idle_at: row.idle_at,
                request_started_at: row.request_started_at,
                turn_started_at: row.turn_started_at,
            };
            let stored = row.stored_status.map(|s| s.0);
            sync_status(
                &self.event_bus,
                row.thread_id,
                &candidate,
                rule,
                &anchors,
                stored,
            )
            .await;
        }
        clear_stale_statuses(&self.event_bus, &candidates).await;
    }

    async fn keep_alive_mode(&self) -> CacheKeepAliveMode {
        CacheKeepAliveMode::parse(
            &crate::core::prefs::CACHE_KEEP_ALIVE_MODE
                .read(&self.pool)
                .await,
        )
    }

    /// One thread's row as the selection reads it. The tick and the cache-warm
    /// line both build their candidate here, so they never disagree.
    async fn candidate_of(
        &self,
        row: &CandidateRow,
        wait: Option<(DateTime<Utc>, KeepWarmKinds)>,
        now: DateTime<Utc>,
    ) -> Candidate {
        let agent = if !row.is_coding_agent {
            Agent::Lucidos(self.remembered_requests.state(row.thread_id))
        } else {
            match CodingAgent::parse(row.coding_agent.as_deref().unwrap_or("")) {
                CodingAgent::ClaudeCode => Agent::ClaudeCode,
                CodingAgent::Codex => Agent::Codex,
            }
        };
        Candidate {
            agent,
            claim: self.keep_alive_claim(row.thread_id).await,
            awaiting_user: row.awaiting_user,
            parked: self.parked_of(row),
            keep_warm_wait_secs: wait.map(|(at, _)| (now - at).num_milliseconds() as f64 / 1000.0),
            keep_warm_kinds: wait.map(|(_, kinds)| kinds).unwrap_or_default(),
            children: row.active_children(),
            secs_since_request: row.secs_since_request,
            secs_since_beat: self.secs_since_keep_alive_beat(row.thread_id),
            cache_ttl: row.cache_ttl(),
            cache: row.cache(),
            turned_off_at: row.turned_off_at,
        }
    }

    /// What the keep-alive does for `thread_id`, for the cache-warm line.
    /// Only a thread that does not exist has no candidate row.
    pub(crate) async fn warm_line_keep_alive(
        &self,
        thread_id: Uuid,
    ) -> Result<WarmLine, sqlx::Error> {
        let Some(row) = read_candidates(&self.pool, &[thread_id])
            .await?
            .into_iter()
            .find(|row| row.thread_id == thread_id)
        else {
            return Ok(WarmLine {
                keep_alive: WarmLineKeepAlive::NotKept {
                    reason: WarmLineNotKept::NothingPending,
                },
                reread_usd: None,
            });
        };
        let durations = self.keep_alive_durations.current();
        let rule = GiveUpRule::of(self.keep_alive_mode().await, &durations);
        let candidate = self.candidate_of(&row, None, Utc::now()).await;
        let anchors = Anchors {
            keep_warm_armed_at: None,
            idle_at: row.idle_at,
            request_started_at: row.request_started_at,
            turn_started_at: row.turn_started_at,
        };
        let spent = match (parked_wait(&candidate), row.turn_started_at) {
            (Some(_), Some(since)) => read_capture_spent(&self.pool, thread_id, since).await?,
            _ if candidate.turned_off_at.is_some() => Spent {
                context_tokens: crate::engine::successor::thread_context(&self.pool, thread_id)
                    .await?
                    .input_tokens
                    .unwrap_or(0),
                ..Spent::default()
            },
            _ => Spent::default(),
        };
        let keep_alive = cache_warm_line(&candidate, rule, &anchors, spent);
        Ok(WarmLine {
            reread_usd: keep_alive.reread_tokens().and_then(|tokens| {
                reread_usd(tokens, row.last_write_model.as_deref()?, candidate.ttl())
            }),
            keep_alive,
        })
    }

    /// Busy when a session, a hold or a turn has the thread.
    async fn keep_alive_claim(&self, thread_id: Uuid) -> ThreadClaim {
        let held = self
            .agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .is_some_and(crate::engine::agent_session::spawn_claim::holds_thread);
        let turning = self.active_threads.lock().unwrap().contains_key(&thread_id);
        if held || turning {
            ThreadClaim::Busy
        } else {
            ThreadClaim::Free
        }
    }

    /// The card the row's session is parked on, timed from the turn request
    /// that asked.
    fn parked_of(&self, row: &CandidateRow) -> Option<Parked> {
        Some(Parked {
            card: row.parked_card()?,
            secs: row.secs_since_turn_request?,
            copy_ready: self.remembers_cc_system_prompt(row.thread_id),
        })
    }

    fn secs_since_keep_alive_beat(&self, thread_id: Uuid) -> Option<f64> {
        self.keep_alive_beats
            .lock()
            .unwrap()
            .get(&thread_id)
            .map(|at| at.elapsed().as_secs_f64())
    }

    /// Write the hidden anchor, then resume the session through the same route
    /// a wait re-entry takes. A busy thread skips the beat inside that route.
    async fn send_keep_alive(self: &Arc<Self>, thread_id: Uuid) {
        self.keep_alive_beats
            .lock()
            .unwrap()
            .insert(thread_id, std::time::Instant::now());
        let anchor = ThreadEvent::PromptInjected {
            text: KEEP_ALIVE_PROMPT.to_string(),
            mode: ActorMode::Engine,
            origin: Some(MessageOrigin::engine(EngineReason::CacheKeepAlive)),
            injected_message_id: None,
            delivered_event_id: None,
        };
        let emitted = self
            .event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: anchor,
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            })
            .await;
        let anchor_id = match emitted {
            Ok(Some(result)) => result.event_id,
            Ok(None) => return,
            Err(e) => {
                crate::log!("[KeepAlive] Could not write the anchor for thread {thread_id}: {e}");
                return;
            }
        };
        crate::log!("[KeepAlive] Resuming thread {thread_id} to keep its prompt cache warm");
        let engine = self.clone();
        tokio::spawn(async move {
            if let Err(e) = engine
                .process_message_with_steps(
                    KEEP_ALIVE_PROMPT,
                    None,
                    None,
                    None,
                    None,
                    None, // provider_override
                    None,
                    None,
                    Some(true),
                    None,
                    Some(thread_id),
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    ActorMode::Engine,
                    None,
                    None,
                    Some(PreEmittedOrigin::KeepAlive(anchor_id)),
                    None,
                    None,
                    None,
                    crate::engine::FollowUpUrgency::Normal,
                    None,
                )
                .await
            {
                crate::log!("[KeepAlive] Keep-alive turn failed for thread {thread_id}: {e}");
            }
        });
    }

    /// Beat a parked session by asking a copy of it, in the background. The
    /// parked session, its transcript and its card never hear of it (I1).
    fn send_keep_alive_copy(self: &Arc<Self>, thread_id: Uuid) {
        self.keep_alive_beats
            .lock()
            .unwrap()
            .insert(thread_id, std::time::Instant::now());
        crate::log!(
            "[KeepAlive] Asking a copy of parked thread {thread_id} to keep its prompt cache warm"
        );
        let engine = self.clone();
        tokio::spawn(async move { engine.ask_keep_alive_copy(thread_id).await });
    }

    /// Ask the copy, then record what it cost. The capture's `duration_ms`
    /// dates the copy's launch, which the next beat's timing reads.
    async fn ask_keep_alive_copy(&self, thread_id: Uuid) {
        let copy = match self.session_copy(thread_id).await {
            Ok(copy) => copy,
            Err(e) => {
                crate::log!("[KeepAlive] No session copy for thread {thread_id}: {e:?}");
                return;
            }
        };
        // Checked at the tick too. An eviction since then would cost a full
        // rewrite for nothing.
        if !copy.shares_the_prefix() {
            crate::log!("[KeepAlive] Thread {thread_id} lost its appended prompt; no copy beat");
            return;
        }
        let launched = std::time::Instant::now();
        let ask = crate::runtime::claude_code::SessionCopyAsk {
            max_turns: KEEP_ALIVE_COPY_MAX_TURNS,
            deadline: tokio::time::Instant::now() + KEEP_ALIVE_COPY_TIMEOUT,
            timeout_message: format!(
                "the copy did not answer within {} seconds",
                KEEP_ALIVE_COPY_TIMEOUT.as_secs()
            ),
        };
        let args = copy.spawn_args(self.workspace_path(), thread_id);
        let reply =
            match crate::runtime::claude_code::ask_session_copy(args, KEEP_ALIVE_PROMPT, &[], ask)
                .await
            {
                Ok(reply) => reply,
                Err(e) => {
                    crate::log!("[KeepAlive] Copy beat failed for thread {thread_id}: {e}");
                    return;
                }
            };
        let detail = crate::engine::model_call::CallDetail {
            reasoning_effort: copy.effort().map(str::to_string),
            duration_ms: u64::try_from(launched.elapsed().as_millis()).ok(),
            ..Default::default()
        };
        crate::engine::AuxCapture::new(
            &self.event_bus,
            thread_id,
            crate::engine::ContextPurpose::CacheKeepAlive,
        )
        .record_usage_detailed(
            &copy.model_of(&reply),
            KEEP_ALIVE_PROMPT.chars().count(),
            reply.usage,
            detail,
        )
        .await;
        if let Err(e) = reply.answer {
            crate::log!("[KeepAlive] Copy beat for thread {thread_id} ended without a word: {e}");
        }
    }

    /// Send a Lucidos Agent thread's remembered request again, in the
    /// background. It claims no thread and writes no event but its capture.
    /// With every beat slot taken, it waits for the next tick.
    fn send_prefill_beat(self: &Arc<Self>, thread_id: Uuid, cut: PrefillCut) {
        let Some(pair) = self.remembered_requests.pair(thread_id) else {
            return;
        };
        let body = match cut {
            PrefillCut::Stable => pair.stable,
            PrefillCut::Whole => pair.whole,
        };
        let engine = self.clone();
        tokio::spawn(async move {
            let Some(_slot) = engine.remembered_requests.claim_slot() else {
                return;
            };
            engine
                .keep_alive_beats
                .lock()
                .unwrap()
                .insert(thread_id, std::time::Instant::now());
            let provider = engine.current_provider();
            let store = &engine.remembered_requests;
            beat_once(
                &engine.event_bus,
                store,
                provider.as_ref(),
                thread_id,
                &body,
            )
            .await;
        });
    }
}

/// Send one prefill beat, record what it cost, and judge it by its usage
/// block, never its stop reason (ADR 0556). It takes no engine, so it cannot
/// claim the thread or delay a turn.
pub(super) async fn beat_once(
    bus: &EventBus,
    store: &super::RememberedRequests,
    llm: &dyn crate::llm::LlmProvider,
    thread_id: Uuid,
    body: &PrefillBody,
) {
    let Some(provider) = body.provider else {
        return;
    };
    let first = store.unproven(provider);
    let started = std::time::Instant::now();
    let reply = crate::engine::AuxCapture::new(
        bus,
        thread_id,
        crate::engine::ContextPurpose::CacheKeepAlive,
    )
    .prefill(llm, body)
    .await;
    let outcome = match reply {
        Ok(usage) => judge_beat(&usage, body.output_cap),
        Err(e) if first && e.is::<crate::llm::prefill::PrefillRefused>() => {
            crate::log!(
                "[KeepAlive] {} refused its first prefill beat, so it beats no more \
                 until a restart: {e}",
                provider.as_str()
            );
            store.mark_unable(provider);
            return;
        }
        Err(e) => {
            crate::log!("[KeepAlive] Prefill beat failed for thread {thread_id}: {e}");
            return;
        }
    };
    match outcome {
        BeatOutcome::Read => {
            store.mark_proven(provider);
            store.refreshed(thread_id, started);
        }
        BeatOutcome::Talkative => {
            crate::log!(
                "[KeepAlive] A prefill beat on {} wrote output, so it beats no more \
                 until a restart",
                provider.as_str()
            );
            store.mark_unable(provider);
            store.forget(thread_id);
        }
        BeatOutcome::Wrote => {
            crate::log!(
                "[KeepAlive] A prefill beat for thread {thread_id} wrote to the cache, \
                 so its run ends"
            );
            if first {
                store.mark_unable(provider);
            }
            store.forget(thread_id);
        }
    }
}

/// What one prefill beat's usage block says it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BeatOutcome {
    /// It read the entry and wrote nothing worth counting.
    Read,
    /// It generated more output than its body allows.
    Talkative,
    /// It wrote over [`BEAT_WRITE_SHARE`] of its input: the cache moved.
    Wrote,
}

/// The share of a beat's input it may write before the run ends. A beat
/// reads a prefix the turn wrote, so any real write means it no longer
/// matches.
const BEAT_WRITE_SHARE: f64 = 0.02;

pub(super) fn judge_beat(
    usage: &crate::llm::provider::LlmResponse,
    output_cap: Option<u32>,
) -> BeatOutcome {
    if output_cap.is_some_and(|cap| usage.output_tokens.unwrap_or(0) > cap) {
        return BeatOutcome::Talkative;
    }
    let input = f64::from(usage.input_tokens.unwrap_or(0));
    let wrote = f64::from(usage.cache_creation_tokens.unwrap_or(0));
    if wrote > BEAT_WRITE_SHARE * input {
        BeatOutcome::Wrote
    } else {
        BeatOutcome::Read
    }
}

#[cfg(test)]
#[path = "keep_alive_tests.rs"]
mod tests;
