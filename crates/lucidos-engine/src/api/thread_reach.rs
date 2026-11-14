//! May this caller press this button? Two questions, and both are asked here.
//!
//! **Which threads may it aim at?** A thread reaches itself and its own
//! descendants, on its own authority, and nothing else (ADR 0168 clause 3).
//! This is the ladder the messaging edge already had (ADR 0043,
//! `engine::chat::child_follow_up`), widened from direct children to
//! descendants because the archive cascade walks the whole family.
//!
//! **On whose behalf does it act?** Anything wider is the *workspace owner*'s
//! button. A thread may press one while carrying their standing instruction
//! (clauses 4 and 5, `api::standing_instruction`). It may also hold a
//! *permission* the owner granted for the act (ADR 0664,
//! `api::permission_requests`), and spends a once one. This module refuses
//! when none of the three covers the act.
//! Submitting saved owner content always needs an owner instruction or a
//! permission, including inside the caller's subtree (ADR 0651).
//!
//! **Creating a thread is not on the list.** Any caller may create a
//! top-thread, or move its own child to top level (ADR 0475). A handoff
//! starts a session in its own thread. Those three verbs stay in the enum only
//! because stored approval cards name them.
//!
//! The gate is per VERB, never per route. Three verbs arrive by more than one
//! path, and gating the first path of each is how the ungated set grew.
//!
//! **Messaging is reach, not authority.** A follow-up addresses a thread's own
//! direct children, and the *home thread* may address any thread (ADR 0362).
//! [`follow_up_reach`] answers that. It widens no clause-4 verb: the home
//! thread presses an owner button only as any other thread does.
//!
//! **One thing here is still open, and it is recorded rather than left to be
//! rediscovered** in `docs/plans/2026-08-30-a-thread-acts-in-its-own-subtree.md`.
//! Taking an owner's standing apply back has no gate, and clause 4 has no verb
//! for it: it classifies pressing an owner button, not revoking one.
//!
//! - **The caller is authenticated, never asserted.** It is the prefix of the
//!   thread-bound origin token, which is HMAC-covered (`api::actor`).
//! - **The relationship is read, never claimed.** The caller passes a target
//!   id and says nothing about how the two are related.
//! - **A refusal is typed, with its status beside the taxonomy**, because
//!   handlers with different body shapes consume it structurally.

use axum::http::{HeaderMap, StatusCode};
use uuid::Uuid;

use crate::api::actor::SubprocessOrigin;
use crate::api::permission_requests::spend_live;
use crate::api::standing_instruction::carries_standing_instruction;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::{Action, PermissionUser};

/// Why a clause-4 verb was refused. Every branch is a refusal: none falls
/// back to a narrower target, and none reports success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ThreadReachError {
    /// The target is neither the caller nor one of its descendants, and the
    /// caller carries no standing instruction from the workspace owner.
    /// Deliberately one variant for a sibling, a parent and an unknown thread,
    /// exactly as `ChildFollowUpError::NotYourChild` is: the caller learns the
    /// thread is out of reach, and nothing about whose it is.
    OutOfReach { target: Uuid, verb: Action },
    /// A verb aimed at the workspace rather than at a thread, by a caller
    /// carrying no standing instruction. Applying a change that names no
    /// thread is the case: the workspace is nobody's subtree.
    NoStandingInstruction(Action),
    /// A verified subprocess with no thread of its own, and not a trigger's
    /// fire either. It has no subtree to scope to and no turn to carry an
    /// instruction, so it is refused rather than handed the whole workspace.
    NoCallerThread(Action),
    /// A thread-bound caller asked to cancel with no `thread_id`, which stops
    /// every thread in the workspace. Refused rather than silently reread as
    /// "cancel yourself": a caller that meant itself can say so, and one that
    /// meant everything must not get it without the owner behind it.
    UnscopedCancel,
    /// The ancestor chain could not be read, so the call fails closed. That
    /// costs the user nothing: only a token-bearing caller runs that query, so
    /// a database hiccup can never reach the Archive or Stop button.
    Unverifiable(String),
    /// A caller with no signed identity, an *unattested caller*, at a verb
    /// only the owner presses (`api::owner_door`).
    Unattested(UnattestedOutcome),
    /// A thread answered a card. Only the owner does, so a thread relays an
    /// answer through a batch the owner confirms (ADR 0664).
    CardAnswerNeedsBatch,
}

/// What became of an unattested caller's owner-only verb.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnattestedOutcome {
    /// It waits for the owner's Allow once.
    Waiting,
    /// Nothing could park it, so it was refused.
    Refused,
    /// It answered a card, which only the owner does.
    CardAnswer,
}

/// What a thread is told about the authority it does not have. Appended to
/// every refusal below, so no site can spell the remedy its own way.
///
/// It names the *workspace owner* and not a parent thread. Most threads in a
/// workspace have none, so the old sentence sent the majority of callers to
/// something with no row (ADR 0168 clause 1).
const OWNERS_TO_PRESS: &str = "Anything wider than your own subtree is the workspace owner's \
     to press. A thread may press it while carrying their standing instruction (a turn \
     they opened, or a trigger firing they authorized), or with a permission they granted \
     for the act. An agent asks for one with `lucidos ask-permission <verb>`. Otherwise \
     report what you found and let the owner decide.";

impl ThreadReachError {
    /// HTTP status, beside the taxonomy so neither handler's mapping can drift
    /// from it.
    pub(crate) fn status_code(&self) -> StatusCode {
        match self {
            Self::OutOfReach { .. }
            | Self::NoStandingInstruction(_)
            | Self::NoCallerThread(_)
            | Self::UnscopedCancel
            | Self::CardAnswerNeedsBatch
            | Self::Unattested(UnattestedOutcome::Refused | UnattestedOutcome::CardAnswer) => {
                StatusCode::FORBIDDEN
            }
            Self::Unattested(UnattestedOutcome::Waiting) => StatusCode::ACCEPTED,
            Self::Unverifiable(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Machine-readable slug for the archive route, whose rejection bodies are
    /// `{reason, ...}` rather than `{error}`.
    pub(crate) fn reason(&self) -> &'static str {
        match self {
            Self::OutOfReach { .. } => "out_of_reach",
            Self::NoStandingInstruction(_) => "no_standing_instruction",
            Self::NoCallerThread(_) => "no_caller_thread",
            Self::UnscopedCancel => "unscoped_cancel",
            Self::CardAnswerNeedsBatch => "card_answer_needs_batch",
            Self::Unverifiable(_) => "reach_unverifiable",
            Self::Unattested(UnattestedOutcome::Waiting) => "waiting_for_owner",
            Self::Unattested(_) => "unattested",
        }
    }
}

impl std::fmt::Display for ThreadReachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfReach {
                target,
                verb: Action::SubmitDraft,
            } => write!(
                f,
                "Submitting the saved draft on thread {target} requires the workspace \
                 owner's standing instruction or their permission, even within your own \
                 subtree. An agent asks with `lucidos ask-permission submit-draft \
                 --thread {target} --reason \"<why>\"`, or the owner instructs it in their \
                 own turn."
            ),
            Self::OutOfReach { target, verb } => write!(
                f,
                "Thread {target} is out of your reach, so you cannot {} it. On its own \
                 authority a thread can only {} itself and the sub-threads below it. \
                 {OWNERS_TO_PRESS}",
                verb.word(),
                verb.word()
            ),
            Self::NoStandingInstruction(verb) => write!(
                f,
                "This request is {}, aimed at the workspace rather than at a thread. The \
                 workspace is nobody's subtree. {OWNERS_TO_PRESS}",
                verb.act()
            ),
            Self::NoCallerThread(verb) => write!(
                f,
                "This request comes from a Lucidos subprocess with no thread of its \
                 own, so {} has no subtree to stay inside. {OWNERS_TO_PRESS}",
                verb.act()
            ),
            Self::UnscopedCancel => write!(
                f,
                "Cancelling with no thread_id stops every thread in the workspace, \
                 which is the user's own Stop. Name a thread instead: your own id, or \
                 one of the sub-threads below it. {OWNERS_TO_PRESS}"
            ),
            Self::CardAnswerNeedsBatch => f.write_str(
                "Only the workspace owner answers a card, so no thread answers one: not its \
                 own, not a sub-thread's, and not on a turn the owner opened. To answer for \
                 the owner, raise a batch on your own thread with `lucidos permissions batch`, \
                 naming the exact pick. It runs once the owner confirms it.",
            ),
            Self::Unverifiable(msg) => write!(
                f,
                "Could not check whether that thread is within your reach, so the \
                 request was refused: {msg}"
            ),
            Self::Unattested(outcome) => f.write_str(match outcome {
                UnattestedOutcome::Waiting => crate::api::owner_door::WAITING_FOR_THE_OWNER,
                UnattestedOutcome::Refused => crate::api::actor::NOT_ATTESTED_AS_OWNER,
                UnattestedOutcome::CardAnswer => crate::api::owner_door::UNATTESTED_CARD_ANSWER,
            }),
        }
    }
}

impl std::error::Error for ThreadReachError {}

/// The refusal as the `{"error": msg}` body an `ApiError` handler returns, so a
/// gated route writes `?` rather than re-deriving the status and the text.
///
/// Spelling that mapping per site is one status lookup away from a route
/// answering 500 where its neighbours answer 403. The archive route keeps its
/// own mapping: its rejection bodies are `{reason, ...}`, which is what
/// [`ThreadReachError::reason`] is for.
impl From<ThreadReachError> for super::error::ApiError {
    fn from(e: ThreadReachError) -> Self {
        Self::new(e.status_code(), e.to_string())
    }
}

/// The ladder, and only the ladder: is `target` the caller itself, or one of
/// its descendants? Clause 3, saying nothing about clause 5.
///
/// Self is answered with no query at all. Otherwise one recursive CTE walks
/// from the target UPWARD, so the cost is the depth of the tree rather than its
/// width: a parent of ten children pays what a parent of one pays.
///
/// A bool rather than a refusal, because [`refuse_without_authority`] asks a
/// second question after a `false` and must not read it as an answer yet. An
/// `Err` stays an `Err`: a ladder nobody could read must not fall through to
/// the wider path.
///
/// Takes the pool rather than `&self`, for the reason
/// `authorize_child_follow_up` does: the whole check is one query and a ladder,
/// so it is testable without standing up an engine.
pub(in crate::api) async fn authorize_thread_reach(
    pool: &sqlx::PgPool,
    caller_thread_id: Uuid,
    target_thread_id: Uuid,
) -> Result<bool, ThreadReachError> {
    if caller_thread_id == target_thread_id {
        return Ok(true);
    }

    let reachable =
        crate::core::store::is_thread_ancestor(pool, caller_thread_id, target_thread_id)
            .await
            .map_err(|e| {
                crate::log!(
                    "[ThreadReach] Could not read the ancestors of {} for caller {}: {}",
                    target_thread_id,
                    caller_thread_id,
                    e
                );
                ThreadReachError::Unverifiable(e.to_string())
            })?;

    Ok(reachable)
}

/// How a follow-up reaches its target, as [`follow_up_reach`] reads it. The
/// wire spells it `own-child` or `home`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FollowUpReach {
    /// The target is one of the caller's own direct children.
    OwnChild,
    /// The caller is the home thread, and the target is not its child. Its
    /// reply goes to its own parent, if any, never back to the home thread.
    Home,
}

/// May `caller` follow up on a thread whose parent is `target_parent`? `None`
/// means the target is out of reach.
///
/// A thread reaches its own direct children (ADR 0043). The home thread also
/// reaches every other thread, coding-agent threads included (ADR 0362). No
/// other thread gains anything, so a sibling stays out of reach.
pub(crate) async fn follow_up_reach(
    pool: &sqlx::PgPool,
    caller: Uuid,
    target_parent: Option<Uuid>,
) -> Result<Option<FollowUpReach>, sqlx::Error> {
    if target_parent == Some(caller) {
        return Ok(Some(FollowUpReach::OwnChild));
    }
    Ok(crate::engine::home_thread::is_home_thread(pool, caller)
        .await?
        .then_some(FollowUpReach::Home))
}

/// Refuse anyone but the attested owner pressing either Always-allow grant on
/// a permission card.
///
/// Both widen what every future session may do without asking, so they stay
/// the owner's, pressed on screen (ADR 0362). No standing instruction lifts
/// this, and voice withholds them for the same reason (`voice::decision`).
/// Allow once, this thread's allow and Deny are not affected.
fn require_owner_for_always_allow(
    headers: &HeaderMap,
    persist_scope: Option<crate::engine::claude_code::AllowScope>,
) -> Result<(), super::error::ApiError> {
    use crate::engine::claude_code::AllowScope;
    if !matches!(persist_scope, Some(AllowScope::Narrow | AllowScope::Broad)) {
        return Ok(());
    }
    crate::api::actor::require_attested_owner(
        headers,
        "An agent cannot press Always allow. It widens what every future session \
         may do without asking, so only the workspace owner grants it, on screen. \
         Allow once, allow for this thread, or deny instead.",
        crate::api::owner_door::OnUnattested::Refuse,
    )
}

/// Refuse anyone but the attested owner changing a permission grant through
/// Settings: a grant file, or an MCP server's auto-approve flag.
///
/// The same reasoning as [`require_owner_for_always_allow`]. These routes write
/// the very grants that button writes, so a caller refused on the card must not
/// reach them here (ADR 0362).
pub(in crate::api) fn require_owner_for_permission_grant(
    headers: &HeaderMap,
) -> Result<(), super::error::ApiError> {
    crate::api::actor::require_attested_owner(
        headers,
        "An agent cannot change a permission grant. It decides what every future \
         session may do without asking, so only the workspace owner changes it, in \
         Settings.",
        crate::api::owner_door::OnUnattested::Ask(crate::api::owner_door::OwnerAct::new(
            "change what agents may run without asking",
        )),
    )
}

/// Refuse an agent answering a permission card it has no authority for.
///
/// Two checks, the widest grants first. No agent presses Always allow; a
/// denial carrying a scope grants nothing, so it is not one. Any other answer
/// is a card answer on the card's thread, which no thread gives on its own
/// authority (ADR 0664). A thread relays one through a batch instead.
///
/// `card_thread` is `None` for a card nothing is waiting on, and the route
/// then answers its own 404.
pub(in crate::api) async fn refuse_permission_answer(
    bus: &EventBus,
    headers: &HeaderMap,
    card_thread: Option<Uuid>,
    allowed: bool,
    persist_scope: Option<crate::engine::claude_code::AllowScope>,
) -> Result<(), super::error::ApiError> {
    if allowed {
        require_owner_for_always_allow(headers, persist_scope)?;
    }
    let Some(thread) = card_thread else {
        return Ok(());
    };
    refuse_without_authority(bus, headers, Some(thread), Action::ResolvePermission)
        .await
        .map_err(Into::into)
}

/// Refuse a clause-4 verb this caller has no authority for.
///
/// Two questions in order, and the cheap one first. Inside its own subtree a
/// thread acts on its own authority, which costs one query at most (clause 3).
/// Anything wider is the owner's button, requiring a standing instruction or
/// a permission. Saved-draft submission takes that owner path even
/// inside the caller's subtree (ADR 0651).
///
/// `target` is optional because some acts aim at no single thread: `POST
/// /api/v1/chat/cancel` with no thread id means "everything", and a change
/// naming no thread belongs to the workspace. Both are wider than any subtree,
/// so both take the standing-instruction path.
///
/// Runs BEFORE the handler writes anything, so a refusal leaves no half-applied
/// cascade, no merged branch and no cancel-stamped question card behind.
///
/// ## A caller presenting no token keeps its reach
///
/// That is the user's own device, the local API surface and the e2e suites,
/// and it is the answer `threads::actions::refuse_event_waits_for_another_thread`
/// already gives. What this buys is that a caller presenting its credential is
/// bound by it.
///
/// The residual it leaves is narrower than it was. A subprocess dropping its
/// token used to read as an ordinary local caller. `api::mutating_gate` now
/// refuses one presenting NO credential at all (ADR 0169), and every clause-4
/// verb is a mutating method, so that layer answers first. What is left is a
/// caller holding one of the other three credentials. Each names a device or
/// the machine rather than a thread, so none has a reach to weigh.
pub(in crate::api) async fn refuse_without_authority(
    bus: &EventBus,
    headers: &HeaderMap,
    target: Option<Uuid>,
    verb: Action,
) -> Result<(), ThreadReachError> {
    let SubprocessOrigin::Subprocess {
        source_thread_id,
        emitting_trigger_id,
        ..
    } = crate::api::actor::subprocess_origin(headers)
    else {
        return refuse_unattested(bus.pool(), headers, target, verb).await;
    };
    weigh_authority(
        bus,
        source_thread_id,
        emitting_trigger_id.as_deref(),
        target,
        verb,
    )
    .await
}

/// The owner's verbs, for a caller holding no thread token. Apply, discard and
/// revert wait for the owner's Allow once when the caller is unattested. A
/// card answer from one is refused, since only the owner answers a card.
async fn refuse_unattested(
    pool: &sqlx::PgPool,
    headers: &HeaderMap,
    target: Option<Uuid>,
    verb: Action,
) -> Result<(), ThreadReachError> {
    use crate::api::actor::{owner_attestation, OwnerAttestation};
    use crate::api::owner_door::{ask, thread_named, OwnerAct};
    if owner_attestation(headers) != OwnerAttestation::Unattested {
        return Ok(());
    }
    match verb {
        Action::Apply | Action::Discard | Action::Revert => {
            let aimed_at = match target {
                Some(thread) => thread_named(pool, thread).await,
                None => "the workspace".to_string(),
            };
            let act = OwnerAct::new(format!("{} {aimed_at}", verb.word())).on_thread(target);
            Err(ThreadReachError::Unattested(
                if ask(act).status == StatusCode::ACCEPTED {
                    UnattestedOutcome::Waiting
                } else {
                    UnattestedOutcome::Refused
                },
            ))
        }
        Action::AnswerQuestion | Action::ResolvePermission => {
            Err(ThreadReachError::Unattested(UnattestedOutcome::CardAnswer))
        }
        _ => Ok(()),
    }
}

/// [`refuse_without_authority`], for a caller that is already IN the thread.
///
/// An LLM tool runs in-process and carries no headers, so it names its own
/// thread instead of presenting a token for one. The rule below is the same
/// one, which is the point: Apply reached by a tool and Apply reached by a
/// route must not answer differently.
///
/// No trigger id, because that field exists to name a fire that has no thread
/// of its own. A tool call always has one, and a trigger thread's own turn
/// carries the firing already.
pub(crate) async fn refuse_thread_without_authority(
    bus: &EventBus,
    caller_thread_id: Uuid,
    target: Option<Uuid>,
    verb: Action,
) -> Result<(), ThreadReachError> {
    weigh_authority(bus, Some(caller_thread_id), None, target, verb).await
}

/// Which authority of its own a caller holds for an act, before any
/// *permission* is weighed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HeldAuthority {
    /// The verb is retired and needs no authority.
    NoneNeeded,
    /// The target is the caller or one of its descendants (clause 3).
    OwnSubtree,
    /// The caller carries the owner's standing instruction (clause 5).
    StandingInstruction,
}

/// Clauses 3 and 5, over the two authenticated facts about the caller.
///
/// Shared by the gate and by the permission request route. That route
/// refuses to ask the owner for an act the caller can already do.
pub(crate) async fn authority_held(
    pool: &sqlx::PgPool,
    source_thread_id: Option<Uuid>,
    emitting_trigger_id: Option<&str>,
    target: Option<Uuid>,
    verb: Action,
) -> Result<Option<HeldAuthority>, ThreadReachError> {
    // A snapshot is workspace-scoped. Thread reach and a turn's standing
    // instruction cannot stand in for the restore policy or its bound card.
    if verb == Action::RestoreSnapshot {
        return Ok(None);
    }
    if !verb.needs_authority() {
        return Ok(Some(HeldAuthority::NoneNeeded));
    }
    // A card answer is the owner's own pick, so neither clause covers it.
    if verb.answers_a_card() {
        return Ok(None);
    }
    // Sending saved owner content always needs the owner's instruction or
    // permission, even when the caller owns the target subtree (ADR 0651).
    // A threadless caller likewise skips straight to the owner's question.
    if let (Some(caller), Some(target)) = (source_thread_id, target) {
        if verb != Action::SubmitDraft && authorize_thread_reach(pool, caller, target).await? {
            return Ok(Some(HeldAuthority::OwnSubtree));
        }
    }
    if carries_standing_instruction(pool, source_thread_id, emitting_trigger_id).await {
        return Ok(Some(HeldAuthority::StandingInstruction));
    }
    Ok(None)
}

/// The rule itself: the caller's own authority first, then a live permission
/// for this act and target, which the check spends when it is once (ADR 0664).
async fn weigh_authority(
    bus: &EventBus,
    source_thread_id: Option<Uuid>,
    emitting_trigger_id: Option<&str>,
    target: Option<Uuid>,
    verb: Action,
) -> Result<(), ThreadReachError> {
    let held = authority_held(
        bus.pool(),
        source_thread_id,
        emitting_trigger_id,
        target,
        verb,
    )
    .await?;
    if let Some(held) = held {
        if held == HeldAuthority::StandingInstruction {
            crate::log!(
                "[ThreadReach] Allowed on the owner's standing instruction: caller {:?} may {} {:?}",
                source_thread_id,
                verb.word(),
                target
            );
        }
        return Ok(());
    }

    // A thread verb's permission names its target thread, so an unscoped act
    // matches none. Its card promises the act to a thread, so a threadless
    // caller holds none. Saved-draft acceptance spends its grant in its own
    // transaction (ADR 0651).
    if let (Some(caller), Some(target), false) =
        (source_thread_id, target, verb == Action::SubmitDraft)
    {
        let user = PermissionUser {
            thread_id: Some(caller),
            trigger_id: emitting_trigger_id.map(str::to_owned),
        };
        if spend_live(bus, &user, verb, &target.to_string()).await {
            crate::log!(
                "[ThreadReach] Allowed on a permission: caller {:?} may {} {}",
                source_thread_id,
                verb.word(),
                target
            );
            return Ok(());
        }
    }

    crate::log!(
        "[ThreadReach] Refused: caller {:?} tried to {} {:?} with no standing instruction",
        source_thread_id,
        verb.word(),
        target
    );
    Err(match (source_thread_id, target, verb) {
        (_, _, verb) if verb.answers_a_card() => ThreadReachError::CardAnswerNeedsBatch,
        (None, _, verb) => ThreadReachError::NoCallerThread(verb),
        (Some(_), Some(target), verb) => ThreadReachError::OutOfReach { target, verb },
        (Some(_), None, Action::Cancel) => ThreadReachError::UnscopedCancel,
        (Some(_), None, verb) => ThreadReachError::NoStandingInstruction(verb),
    })
}

#[cfg(test)]
#[path = "thread_reach_tests.rs"]
mod tests;
