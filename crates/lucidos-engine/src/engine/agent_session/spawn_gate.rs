//! The spawn gate: who may start a coding-agent run.
//!
//! Every coding-agent session holds the workspace's grants, so the trust
//! decision sits here, before the process starts. [`spawn_provenance`] walks
//! the thread's provenance to the work's cause. A run caused or fed by a
//! trigger starts only when that trigger holds its *coding-agent grant*. A
//! webhook, the host system, or an agent with no thread to trace never
//! starts one.
//!
//! The walk follows provenance, not the callback tree. A top spawn traces
//! back to the thread that caused it, and so does a moved child. Neither
//! launders a trigger's run into an owner's.
//!
//! A caller with no signed identity, an *unattested caller*, starts a run only
//! on the owner's Allow once. The gate parks the run on a card in its thread,
//! and the Allow starts it (`engine::unattested_act`). A parked run is waiting,
//! not refused: the card holds the turn, and nothing writes a failure.
//!
//! A call from another workspace carries that workspace's own verdict on its
//! thread ([`spawn_verdict`]), under a gateway stamp. Only the caller can read
//! its provenance, so the gate here honors the verdict and never upgrades it.
//!
//! A refusal is never silent. It ends the turn with `ResponseFailed`, and a
//! trigger's refusal also notifies, naming the trigger and the missing grant.
//! A run parked on the owner's card returns [`StartAwaitsOwner`] instead.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::sync::RwLock;

use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, EngineReason, EventChannel, EventMeta, MessageOrigin, SpawnVerdict, ThreadDirection,
    ThreadEvent, UnattestedAct, UnattestedRequest,
};
use crate::engine::unattested_act;
use crate::scheduler::notifications::Tap;
use crate::scheduler::user_tasks::{
    emit_trigger_notification, trigger_failure_title, TriggerNotification,
};
use crate::triggers::TriggerConfig;

/// Hard cap on a provenance walk, so a malformed chain or a cycle cannot
/// loop forever. Real spawn trees are shallow.
const MAX_ANCESTRY_HOPS: usize = 16;

/// The cause at the root of a thread's provenance.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SpawnRoot {
    /// A person, the CLI, an API caller, another workspace, or the engine on
    /// its own work.
    Owner,
    /// A scheduler-fired trigger run.
    Trigger {
        trigger_id: String,
        trigger_name: Option<String>,
    },
    /// A caller with no signed identity: no origin token, no gateway stamp.
    /// Its run waits for the owner's Allow once.
    Unattested(MessageOrigin),
    /// A cause no grant can cover.
    Refused(RefusedOrigin),
    /// Another workspace's gate refused the thread that called in.
    RefusedByCaller { workspace: String, reason: String },
}

/// A root that never starts a coding-agent run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefusedOrigin {
    Webhook,
    System,
    /// An agent with no thread to trace it to.
    Agent,
    /// The walk hit its depth cap or a cycle.
    Untraceable,
}

/// Read a thread's *originating* `MessageOrigin`: the earliest
/// `TriggerStarted` or `MessageReceived` that carries one. `None` when no
/// such event does, which only a legacy row or a thread of another
/// workspace gives.
///
/// Also returns that row's `parent_thread_id`, the *callback linkage*. A
/// `relation: "top"` spawn stamps a `ThreadLink` origin with no linkage. An
/// origin that does not parse is an error, never a legacy row.
async fn fetch_thread_origin_and_linkage(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<Option<(MessageOrigin, Option<Uuid>)>, Box<dyn std::error::Error + Send + Sync>> {
    let row: Option<(serde_json::Value, Option<Uuid>)> = sqlx::query_as(
        "SELECT payload->'origin', (payload->>'parent_thread_id')::uuid FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('TriggerStarted', 'MessageReceived') \
           AND payload ? 'origin' \
           AND payload->'origin' != 'null'::jsonb \
         ORDER BY sequence ASC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    let Some((v, parent_thread_id)) = row else {
        return Ok(None);
    };
    Ok(Some((serde_json::from_value(v)?, parent_thread_id)))
}

/// A trigger by id, with the name its fire recorded.
type FiredTrigger = (String, Option<String>);

/// Every trigger whose fire entered `thread_id`, read off the events that
/// carry input. A continue key lets one trigger's fire into another
/// trigger's thread, and that input steers the thread's agents too.
async fn fetch_fired_triggers(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<Vec<FiredTrigger>, sqlx::Error> {
    sqlx::query_as(
        "SELECT DISTINCT ON (o->'reason'->>'trigger_id') \
                o->'reason'->>'trigger_id', o->'reason'->>'trigger_name' FROM ( \
           SELECT COALESCE(payload->'origin', payload->'actor') AS o FROM events \
           WHERE thread_id = $1 AND event_type IN \
             ('TriggerStarted', 'MessageReceived', 'PromptInjected', 'UserQuestionAnswered') \
         ) e \
         WHERE o->>'kind' = 'engine' AND o->'reason'->>'kind' = 'scheduler' \
           AND o->'reason'->>'trigger_id' IS NOT NULL",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
}

/// One step of the walk: where a thread's origin leads.
enum Step {
    Root(SpawnRoot),
    Hop(Uuid),
}

fn step(origin: MessageOrigin, linkage: Option<Uuid>) -> Step {
    match origin {
        MessageOrigin::SavedDraft {
            submitted_by_thread_id,
            ..
        } => Step::Hop(submitted_by_thread_id),
        MessageOrigin::Device { .. } => Step::Root(SpawnRoot::Owner),
        MessageOrigin::Api {
            mode,
            source_thread_id,
            emitting_trigger_id,
            ..
        } => match (mode, source_thread_id, emitting_trigger_id) {
            (ActorMode::Human, _, _) => Step::Root(SpawnRoot::Owner),
            (_, Some(source), _) => Step::Hop(source),
            // A scheduled script: its token names its trigger and no thread.
            (_, None, Some(trigger_id)) => Step::Root(SpawnRoot::Trigger {
                trigger_id,
                trigger_name: None,
            }),
            // A local process outside any thread: the engine's own machine.
            (ActorMode::Engine, None, None) => Step::Root(SpawnRoot::Owner),
            (ActorMode::Agent, None, None) => Step::Root(SpawnRoot::Refused(RefusedOrigin::Agent)),
        },
        // Another workspace, which judged its own thread. Its verdict roots
        // the walk here, since its thread has no origin in this database.
        MessageOrigin::Workspace {
            verdict: Some(ref verdict),
            ref workspace,
            ..
        } => Step::Root(match verdict {
            SpawnVerdict::Allowed => SpawnRoot::Owner,
            SpawnVerdict::Refused { reason } => SpawnRoot::RefusedByCaller {
                workspace: workspace.clone(),
                reason: reason.clone(),
            },
            SpawnVerdict::Unattested => SpawnRoot::Unattested(origin.clone()),
        }),
        // A row from before stamps keeps its old reading (invariant 11).
        MessageOrigin::Workspace {
            verdict: None,
            thread_id,
            ..
        } => match thread_id {
            Some(caller) => Step::Hop(caller),
            None => Step::Root(SpawnRoot::Owner),
        },
        MessageOrigin::ThreadLink {
            thread_id,
            direction,
            ..
        } => match direction {
            // A top spawn has no linkage, but its origin still names the
            // thread that caused it.
            ThreadDirection::Parent => Step::Hop(linkage.unwrap_or(thread_id)),
            ThreadDirection::Child => Step::Root(SpawnRoot::Refused(RefusedOrigin::Agent)),
        },
        MessageOrigin::Engine { reason } => Step::Root(root_of_engine_reason(reason)),
        MessageOrigin::Agent { .. } => Step::Root(SpawnRoot::Refused(RefusedOrigin::Agent)),
        MessageOrigin::Webhook { .. } => Step::Root(SpawnRoot::Refused(RefusedOrigin::Webhook)),
        caller @ MessageOrigin::UnattestedCaller { .. } => {
            Step::Root(SpawnRoot::Unattested(caller))
        }
        MessageOrigin::System => Step::Root(SpawnRoot::Refused(RefusedOrigin::System)),
    }
}

/// Only a trigger's own run roots in that trigger. Every other reason is the
/// engine at work for the owner. The match names every variant, so a new
/// reason fails to compile until someone classifies it.
fn root_of_engine_reason(reason: EngineReason) -> SpawnRoot {
    match reason {
        EngineReason::Scheduler {
            trigger_id,
            trigger_name,
            ..
        } => SpawnRoot::Trigger {
            trigger_id,
            trigger_name,
        },
        EngineReason::MergeConflict
        | EngineReason::MainCheckRed
        | EngineReason::MainCheckFlaky
        | EngineReason::PluginAutoUpdate { .. }
        | EngineReason::HardenRetrigger
        | EngineReason::MissingHardening
        | EngineReason::MissingPlan
        | EngineReason::ContextCeiling
        | EngineReason::SuccessorSession
        | EngineReason::PluginSetup { .. }
        | EngineReason::PluginUpstreamProposal { .. }
        | EngineReason::StaleSession
        | EngineReason::ArchivedBranchWork
        | EngineReason::UnmergedIntegratedWork
        | EngineReason::SuccessorResidue
        | EngineReason::ContinuationStarted
        | EngineReason::OrphanRecovery
        | EngineReason::EventWait { .. }
        | EngineReason::CacheKeepAlive => SpawnRoot::Owner,
    }
}

/// Who caused a thread's work, and which triggers fed it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Provenance {
    pub(crate) root: SpawnRoot,
    /// Every trigger whose fire entered a thread on the walk. Each one needs
    /// its grant, since its input steers the run as much as the root does.
    pub(crate) fired_triggers: Vec<FiredTrigger>,
}

/// The provenance of `thread_id`. Each thread's originating origin either
/// names the root or leads to the thread that caused it. Only the earliest
/// origin sets the root, so a later message or answer never raises trust.
/// A later trigger fire can only lower it. Saved submissions also retain the
/// submitting sources' provenance and trigger grants. A thread with no recorded origin
/// is a legacy row, read as the owner's. A read that fails is untraceable.
pub(crate) async fn spawn_provenance(pool: &sqlx::PgPool, thread_id: Uuid) -> Provenance {
    let mut fired_triggers: Vec<FiredTrigger> = Vec::new();
    let mut pending = BinaryHeap::from([Reverse((0, thread_id))]);
    let mut seen = HashSet::new();
    let mut root = SpawnRoot::Refused(RefusedOrigin::Untraceable);
    let mut unattested_submitter = None;
    // Saved submissions can feed an existing owner thread. Trace every
    // submitting source and original cause. A later saved send must not
    // launder a trigger, an unattested caller or a refusing workspace into
    // owner authority.
    while let Some(Reverse((depth, source))) = pending.pop() {
        if !seen.insert(source) {
            continue;
        }
        let source_root = walk(pool, source, depth, &mut fired_triggers, &mut pending).await;
        if matches!(
            source_root,
            SpawnRoot::Refused(_) | SpawnRoot::RefusedByCaller { .. }
        ) {
            return Provenance {
                root: source_root,
                fired_triggers,
            };
        }
        if source == thread_id {
            root = source_root;
            continue;
        }
        match source_root {
            SpawnRoot::Trigger {
                trigger_id,
                trigger_name,
            } => note_fired(&mut fired_triggers, (trigger_id, trigger_name)),
            SpawnRoot::Unattested(caller) => {
                unattested_submitter.get_or_insert(caller);
            }
            _ => {}
        }
    }
    // The root holds one cause. An unattested submitter takes it, and a
    // trigger root moves to the fired list, where its grant is still checked.
    if let Some(caller) = unattested_submitter {
        match std::mem::replace(&mut root, SpawnRoot::Unattested(caller)) {
            SpawnRoot::Trigger {
                trigger_id,
                trigger_name,
            } => note_fired(&mut fired_triggers, (trigger_id, trigger_name)),
            SpawnRoot::Unattested(own) => root = SpawnRoot::Unattested(own),
            _ => {}
        }
    }
    Provenance {
        root,
        fired_triggers,
    }
}

fn note_fired(fired: &mut Vec<FiredTrigger>, trigger: FiredTrigger) {
    if !fired.iter().any(|(id, _)| *id == trigger.0) {
        fired.push(trigger);
    }
}

async fn walk(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    depth: usize,
    fired: &mut Vec<FiredTrigger>,
    pending: &mut BinaryHeap<Reverse<(usize, Uuid)>>,
) -> SpawnRoot {
    let mut current = thread_id;
    // Saved submissions have no legacy provenance. Losing a submitting
    // thread or one of its causes must not promote the saved send to Owner.
    let mut require_origin = depth > 0;
    let mut seen: HashSet<Uuid> = HashSet::new();
    for hop in depth..MAX_ANCESTRY_HOPS {
        if !seen.insert(current) {
            break;
        }
        let read = match fetch_fired_triggers(pool, current).await {
            Ok(triggers) => {
                for trigger in triggers {
                    if !fired.iter().any(|(id, _)| *id == trigger.0) {
                        fired.push(trigger);
                    }
                }
                let submitters: Result<Vec<Uuid>,sqlx::Error> = sqlx::query_scalar(
                    "SELECT DISTINCT (payload->'origin'->>'submitted_by_thread_id')::uuid FROM events \
                     WHERE thread_id=$1 AND event_type IN ('MessageReceived','PromptInjected') \
                     AND payload->'origin'->>'kind'='saved_draft'",
                ).bind(current).fetch_all(pool).await;
                match submitters {
                    Ok(submitters) => {
                        pending.extend(submitters.into_iter().map(|id| Reverse((hop + 1, id))));
                        fetch_thread_origin_and_linkage(pool, current).await
                    }
                    Err(error) => Err(error.into()),
                }
            }
            Err(e) => Err(e.into()),
        };
        let (origin, linkage) = match read {
            Ok(Some(found)) => found,
            Ok(None) if require_origin => return SpawnRoot::Refused(RefusedOrigin::Untraceable),
            Ok(None) => return SpawnRoot::Owner,
            Err(e) => {
                crate::log!("[SpawnGate] Could not read the provenance of {current}: {e}");
                return SpawnRoot::Refused(RefusedOrigin::Untraceable);
            }
        };
        require_origin |= matches!(origin, MessageOrigin::SavedDraft { .. });
        match step(origin, linkage) {
            Step::Root(root) => return root,
            Step::Hop(next) => current = next,
        }
    }
    SpawnRoot::Refused(RefusedOrigin::Untraceable)
}

/// What this workspace's gate says about a run that `origin` would cause,
/// for a cross-workspace call to carry. The walk and the grants are the ones a
/// local run on that thread meets.
pub(crate) async fn spawn_verdict(
    pool: &sqlx::PgPool,
    trigger_configs: &RwLock<HashMap<String, TriggerConfig>>,
    origin: MessageOrigin,
) -> SpawnVerdict {
    let provenance = match step(origin, None) {
        Step::Root(root) => Provenance {
            root,
            fired_triggers: Vec::new(),
        },
        Step::Hop(thread_id) => spawn_provenance(pool, thread_id).await,
    };
    verdict_of(provenance, trigger_configs)
}

/// [`spawn_verdict`] for work running in `thread_id` itself, as an in-process
/// tool's call is.
pub(crate) async fn thread_spawn_verdict(
    pool: &sqlx::PgPool,
    trigger_configs: &RwLock<HashMap<String, TriggerConfig>>,
    thread_id: Uuid,
) -> SpawnVerdict {
    verdict_of(spawn_provenance(pool, thread_id).await, trigger_configs)
}

fn verdict_of(
    provenance: Provenance,
    trigger_configs: &RwLock<HashMap<String, TriggerConfig>>,
) -> SpawnVerdict {
    match spawn_refusal(provenance, trigger_configs) {
        None => SpawnVerdict::Allowed,
        Some(SpawnRefusal::Unattested(_)) => SpawnVerdict::Unattested,
        Some(refusal) => SpawnVerdict::Refused {
            reason: refusal.to_string(),
        },
    }
}

/// Why the gate refused a run.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SpawnRefusal {
    UngrantedTrigger {
        trigger_id: String,
        trigger_name: String,
    },
    /// The trigger no longer exists, so nobody can grant it.
    DeletedTrigger {
        trigger_id: String,
        trigger_name: String,
    },
    Origin(RefusedOrigin),
    /// An unattested caller started the run, and no Allow once covers it.
    Unattested(MessageOrigin),
    /// The calling workspace's gate refused its thread, for `reason`.
    CallerWorkspace {
        workspace: String,
        reason: String,
    },
}

impl std::fmt::Display for SpawnRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UngrantedTrigger { trigger_name, .. } => write!(
                f,
                "The trigger \"{trigger_name}\" started or fed this coding agent, but it has \
                 no coding-agent grant, so the run was refused. To allow it, open the trigger \
                 and tick \"Allow coding agents\"."
            ),
            Self::DeletedTrigger { trigger_name, .. } => write!(
                f,
                "The trigger \"{trigger_name}\" started or fed this coding agent, and it no \
                 longer exists, so nobody can grant it. The run was refused. Archive this \
                 thread, and start the work again in a new one."
            ),
            Self::Origin(origin) => {
                let cause = match origin {
                    RefusedOrigin::Webhook => "a webhook",
                    RefusedOrigin::System => "the host system",
                    RefusedOrigin::Agent => "an agent with no thread to trace it to",
                    RefusedOrigin::Untraceable => "a cause the engine could not trace",
                };
                write!(
                    f,
                    "This coding agent was started by {cause}, which can never start a \
                     coding agent, so the run was refused."
                )
            }
            Self::Unattested(_) => std::fmt::Display::fmt(&StartAwaitsOwner, f),
            Self::CallerWorkspace { workspace, reason } => write!(
                f,
                "The workspace \"{workspace}\" started this coding agent, and its own gate \
                 refused the thread that asked, so the run was refused here too. It said: \
                 {reason}"
            ),
        }
    }
}

/// Whether `provenance` may start a coding-agent run. The root's trigger and
/// every trigger that fed the walk need the grant. A trigger missing from the
/// registry was deleted, so nobody can grant it.
pub(crate) fn spawn_refusal(
    provenance: Provenance,
    trigger_configs: &RwLock<HashMap<String, TriggerConfig>>,
) -> Option<SpawnRefusal> {
    let (root_trigger, unattested) = match provenance.root {
        SpawnRoot::Owner => (None, None),
        SpawnRoot::Unattested(caller) => (None, Some(caller)),
        SpawnRoot::Refused(origin) => return Some(SpawnRefusal::Origin(origin)),
        SpawnRoot::RefusedByCaller { workspace, reason } => {
            return Some(SpawnRefusal::CallerWorkspace { workspace, reason })
        }
        SpawnRoot::Trigger {
            trigger_id,
            trigger_name,
        } => (Some((trigger_id, trigger_name)), None),
    };
    root_trigger
        .into_iter()
        .chain(provenance.fired_triggers)
        .find_map(|(trigger_id, trigger_name)| ungranted(trigger_id, trigger_name, trigger_configs))
        .or(unattested.map(SpawnRefusal::Unattested))
}

fn ungranted(
    trigger_id: String,
    trigger_name: Option<String>,
    trigger_configs: &RwLock<HashMap<String, TriggerConfig>>,
) -> Option<SpawnRefusal> {
    let config = trigger_configs
        .read()
        .ok()
        .and_then(|m| m.get(&trigger_id).cloned());
    match config {
        Some(c) if c.coding_agent_grant => None,
        Some(c) => Some(SpawnRefusal::UngrantedTrigger {
            trigger_name: c.name,
            trigger_id,
        }),
        None => Some(SpawnRefusal::DeletedTrigger {
            trigger_name: trigger_name.unwrap_or_else(|| trigger_id.clone()),
            trigger_id,
        }),
    }
}

/// A refused run, after the gate recorded it. The caller pushes `notice`.
pub(crate) struct SpawnRefused {
    pub(crate) refusal: SpawnRefusal,
    pub(crate) notice: Option<TriggerNotification>,
}

/// A run the gate parked on the owner's Allow once card. It is waiting, not
/// failed, so no caller writes a terminator: the card holds the turn.
#[derive(Debug)]
pub(crate) struct StartAwaitsOwner;

impl std::fmt::Display for StartAwaitsOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "A caller Lucidos cannot verify started this coding agent, so it waits for the \
             owner's answer on its card."
        )
    }
}

impl std::error::Error for StartAwaitsOwner {}

/// Whether `error` is a run parked on the owner's card rather than a failure.
pub(crate) fn awaits_owner(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error.is::<StartAwaitsOwner>()
}

/// The error a refused or parked run returns to its caller.
fn refusal_error(refusal: &SpawnRefusal) -> Box<dyn std::error::Error + Send + Sync> {
    match refusal {
        SpawnRefusal::Unattested(_) => Box::new(StartAwaitsOwner),
        _ => refusal.to_string().into(),
    }
}

/// Refuse a run on `thread_id` that its root may not start. The refusal ends
/// the turn anchored at `origin_id` with `ResponseFailed`. A trigger's refusal
/// also records a failure notification for that trigger.
///
/// An unattested caller's run starts when the owner allowed it once, and that
/// run spends the Allow. Only the run the Allow resumed may spend it. Otherwise
/// the run parks on a card, with no terminator, until the owner answers.
pub(crate) async fn enforce_spawn_gate(
    bus: &EventBus,
    trigger_configs: &RwLock<HashMap<String, TriggerConfig>>,
    thread_id: Uuid,
    origin_id: Uuid,
) -> Result<(), SpawnRefused> {
    let provenance = spawn_provenance(bus.pool(), thread_id).await;
    let Some(refusal) = spawn_refusal(provenance, trigger_configs) else {
        return Ok(());
    };
    if let SpawnRefusal::Unattested(caller) = &refusal {
        if unattested_act::origin_is_an_allowed_start(bus.pool(), origin_id).await
            && unattested_act::spend_allowed_start(bus, thread_id).await
        {
            crate::log!("[SpawnGate] Starting thread {thread_id}'s run on the owner's Allow once");
            return Ok(());
        }
        crate::log!("[SpawnGate] Thread {thread_id}'s run waits for the owner: {refusal:?}");
        park_on_owner_card(bus, thread_id, caller.clone()).await;
        return Err(SpawnRefused {
            refusal,
            notice: None,
        });
    }
    crate::log!("[SpawnGate] Refused a coding-agent run on thread {thread_id}: {refusal:?}");
    bus.emit_or_log(
        BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseFailed {
                error: refusal.to_string(),
                spend_cap: None,
                agent_exit: None,
                provider_error: None,
            },
            meta: EventMeta {
                request_event_id: Some(origin_id),
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        },
        "[SpawnGate] ResponseFailed",
    )
    .await;
    let notice = match &refusal {
        SpawnRefusal::UngrantedTrigger {
            trigger_id,
            trigger_name,
        }
        | SpawnRefusal::DeletedTrigger {
            trigger_id,
            trigger_name,
        } => {
            let notice = TriggerNotification::new(
                &trigger_failure_title(trigger_name),
                &refusal.to_string(),
            );
            if let Err(e) =
                emit_trigger_notification(bus, trigger_id, &notice, Some(thread_id), Tap::Modal)
                    .await
            {
                crate::log!("[SpawnGate] Failed to notify that {trigger_id} was refused: {e}");
            }
            Some(notice)
        }
        SpawnRefusal::Origin(_)
        | SpawnRefusal::CallerWorkspace { .. }
        | SpawnRefusal::Unattested(_) => None,
    };
    Err(SpawnRefused { refusal, notice })
}

/// Raise the card an unattested run waits on, unless one already waits.
async fn park_on_owner_card(bus: &EventBus, thread_id: Uuid, caller: MessageOrigin) {
    match unattested_act::start_card_waiting(bus.pool(), thread_id).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            crate::log!("[SpawnGate] Could not read the cards of thread {thread_id}: {e}");
            return;
        }
    }
    let act = UnattestedAct {
        caller,
        request: UnattestedRequest::StartCodingAgent,
    };
    let question = unattested_act::card_question("start a coding agent in this thread", &act);
    if let Err(e) =
        unattested_act::raise_card(bus, thread_id, question, act, EventChannel::ClaudeCode).await
    {
        crate::log!("[SpawnGate] Could not ask the owner to start thread {thread_id}: {e}");
    }
}

impl crate::engine::LucidosEngine {
    /// [`enforce_spawn_gate`] for a run on `thread_id`, pushing a trigger's
    /// failure notice to every device. A parked run returns [`StartAwaitsOwner`].
    pub(crate) async fn gate_coding_agent_spawn(
        &self,
        thread_id: Uuid,
        origin_id: Uuid,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let refused =
            match enforce_spawn_gate(&self.event_bus, &self.trigger_configs, thread_id, origin_id)
                .await
            {
                Ok(()) => return Ok(()),
                Err(refused) => refused,
            };
        if let (Some(notice), Some(engine)) = (&refused.notice, self.try_clone_arc()) {
            notice.send_push(&engine);
        }
        Err(refusal_error(&refused.refusal))
    }
}

#[cfg(test)]
#[path = "spawn_gate_tests.rs"]
mod tests;
